use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::Arc,
};

use git::{ComparisonDiff, ComparisonMode, GitRemote};
use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositorySlug {
    pub owner: String,
    pub name: String,
}

impl RepositorySlug {
    #[must_use]
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }

    #[must_use]
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestLocator {
    pub repository: RepositorySlug,
    pub number: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PullRequestSelector {
    Number(u64),
    Url(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestMetadata {
    pub repository: RepositorySlug,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub draft: bool,
    pub base_ref: String,
    pub base_sha: Arc<str>,
    pub head_ref: String,
    pub head_sha: Arc<str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestDiff {
    pub metadata: PullRequestMetadata,
    pub comparison: ComparisonDiff,
}

#[derive(Debug, Error)]
pub enum GithubError {
    #[error("invalid GitHub pull request selector {0:?}")]
    InvalidSelector(String),

    #[error("the repository has no GitHub remote")]
    NoGithubRemote,

    #[error("no local remote matches GitHub repository {0}")]
    NoMatchingRemote(String),

    #[error("failed to execute gh in {repository}: {source}")]
    Execute {
        repository: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("gh api failed with status {status}: {stderr}")]
    Command { status: i32, stderr: String },

    #[error("GitHub returned invalid pull request JSON: {0}")]
    InvalidResponse(#[from] serde_json::Error),

    #[error("GitHub returned PR #{actual} when #{expected} was requested")]
    UnexpectedPullRequest { expected: u64, actual: u64 },

    #[error("GitHub returned base repository {actual}, expected {expected}")]
    UnexpectedRepository { expected: String, actual: String },

    #[error(
        "the pull request was updated while it was loading (expected head {expected}, fetched {actual})"
    )]
    HeadMoved { expected: String, actual: String },

    #[error(transparent)]
    Git(#[from] git::GitError),
}

#[derive(Clone, Debug)]
pub struct GithubClient {
    gh_executable: PathBuf,
}

impl Default for GithubClient {
    fn default() -> Self {
        Self::new("gh")
    }
}

impl GithubClient {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            gh_executable: executable.into(),
        }
    }

    /// Loads metadata and an exact local Git snapshot for one GitHub pull request.
    ///
    /// # Errors
    ///
    /// Returns [`GithubError`] when the selector/remotes are invalid, `gh` is not
    /// authenticated, the namespaced fetch fails, or the PR moves during loading.
    pub fn load_pull_request(
        &self,
        repository: &Path,
        selector: &PullRequestSelector,
    ) -> Result<PullRequestDiff, GithubError> {
        let root = git::repository_root(repository)?;
        let remotes = git::remotes(&root)?;
        let locator = resolve_selector(selector, &remotes)?;
        let metadata = self.fetch_metadata(&root, &locator)?;
        let remote = select_remote(&remotes, &metadata.repository)
            .ok_or_else(|| GithubError::NoMatchingRemote(metadata.repository.full_name()))?;

        let base_tip_sha = fetch_snapshot(&root, remote, &metadata)?;
        let comparison = git::load_comparison(
            &root,
            &base_tip_sha,
            &metadata.head_sha,
            ComparisonMode::MergeBase,
        )?;

        Ok(PullRequestDiff {
            metadata,
            comparison,
        })
    }

    /// Fetches PR metadata through the authenticated `gh` CLI.
    ///
    /// # Errors
    ///
    /// Returns [`GithubError`] when `gh` fails or returns malformed/unexpected JSON.
    pub fn fetch_metadata(
        &self,
        repository: &Path,
        locator: &PullRequestLocator,
    ) -> Result<PullRequestMetadata, GithubError> {
        let endpoint = format!(
            "repos/{}/{}/pulls/{}",
            locator.repository.owner, locator.repository.name, locator.number
        );
        let output = Command::new(&self.gh_executable)
            .current_dir(repository)
            .args([
                "api",
                "--method",
                "GET",
                &endpoint,
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "X-GitHub-Api-Version: 2022-11-28",
            ])
            .env("GH_PROMPT_DISABLED", "1")
            .output()
            .map_err(|source| GithubError::Execute {
                repository: repository.to_path_buf(),
                source,
            })?;
        let output = successful_output(output)?;
        let response: ApiPullRequest = serde_json::from_slice(&output.stdout)?;
        normalize_metadata(response, locator)
    }
}

/// Parses a canonical github.com pull-request URL.
///
/// # Errors
///
/// Returns [`GithubError::InvalidSelector`] for non-HTTPS, non-GitHub, or malformed URLs.
pub fn parse_pull_request_url(value: &str) -> Result<PullRequestLocator, GithubError> {
    let without_fragment = value.split(['?', '#']).next().unwrap_or(value);
    let path = without_fragment
        .strip_prefix("https://github.com/")
        .ok_or_else(|| GithubError::InvalidSelector(value.to_owned()))?;
    let components = path
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components.len() < 4 || components[2] != "pull" {
        return Err(GithubError::InvalidSelector(value.to_owned()));
    }
    let number = components[3]
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| GithubError::InvalidSelector(value.to_owned()))?;
    validate_slug_component(components[0], value)?;
    validate_slug_component(components[1], value)?;

    Ok(PullRequestLocator {
        repository: RepositorySlug::new(components[0], components[1]),
        number,
    })
}

#[derive(Debug, Deserialize)]
struct ApiPullRequest {
    number: u64,
    title: String,
    html_url: String,
    state: String,
    draft: bool,
    base: ApiPullRequestRef,
    head: ApiPullRequestRef,
}

#[derive(Debug, Deserialize)]
struct ApiPullRequestRef {
    #[serde(rename = "ref")]
    name: String,
    sha: String,
    repo: Option<ApiRepository>,
}

#[derive(Debug, Deserialize)]
struct ApiRepository {
    full_name: String,
}

fn normalize_metadata(
    response: ApiPullRequest,
    locator: &PullRequestLocator,
) -> Result<PullRequestMetadata, GithubError> {
    if response.number != locator.number {
        return Err(GithubError::UnexpectedPullRequest {
            expected: locator.number,
            actual: response.number,
        });
    }
    let base_repository = response
        .base
        .repo
        .as_ref()
        .and_then(|repository| parse_full_name(&repository.full_name))
        .ok_or_else(|| GithubError::UnexpectedRepository {
            expected: locator.repository.full_name(),
            actual: response
                .base
                .repo
                .as_ref()
                .map_or_else(|| "<missing>".to_owned(), |repo| repo.full_name.clone()),
        })?;
    if base_repository != locator.repository {
        return Err(GithubError::UnexpectedRepository {
            expected: locator.repository.full_name(),
            actual: base_repository.full_name(),
        });
    }
    validate_sha(&response.base.sha).map_err(GithubError::InvalidSelector)?;
    validate_sha(&response.head.sha).map_err(GithubError::InvalidSelector)?;

    Ok(PullRequestMetadata {
        repository: locator.repository.clone(),
        number: response.number,
        title: response.title,
        url: response.html_url,
        state: response.state,
        draft: response.draft,
        base_ref: response.base.name,
        base_sha: response.base.sha.into(),
        head_ref: response.head.name,
        head_sha: response.head.sha.into(),
    })
}

fn resolve_selector(
    selector: &PullRequestSelector,
    remotes: &[GitRemote],
) -> Result<PullRequestLocator, GithubError> {
    match selector {
        PullRequestSelector::Url(url) => parse_pull_request_url(url),
        PullRequestSelector::Number(number) if *number > 0 => {
            let repository =
                preferred_remote_repository(remotes).ok_or(GithubError::NoGithubRemote)?;
            Ok(PullRequestLocator {
                repository,
                number: *number,
            })
        }
        PullRequestSelector::Number(number) => {
            Err(GithubError::InvalidSelector(number.to_string()))
        }
    }
}

fn preferred_remote_repository(remotes: &[GitRemote]) -> Option<RepositorySlug> {
    let priority = |name: &str| match name {
        "origin" => 0,
        "upstream" => 1,
        _ => 2,
    };
    let mut candidates = remotes
        .iter()
        .flat_map(|remote| {
            remote
                .urls
                .iter()
                .filter_map(|url| parse_remote_url(url).map(|slug| (priority(&remote.name), slug)))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.0);
    candidates.into_iter().next().map(|candidate| candidate.1)
}

fn select_remote<'a>(remotes: &'a [GitRemote], repository: &RepositorySlug) -> Option<&'a str> {
    let priority = |name: &str| match name {
        "upstream" => 0,
        "origin" => 1,
        _ => 2,
    };
    let mut matches = remotes
        .iter()
        .filter(|remote| {
            remote
                .urls
                .iter()
                .filter_map(|url| parse_remote_url(url))
                .any(|candidate| candidate == *repository)
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|remote| priority(&remote.name));
    matches.first().map(|remote| remote.name.as_str())
}

fn parse_remote_url(value: &str) -> Option<RepositorySlug> {
    let path = if let Some(path) = value.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = value.strip_prefix("https://github.com/") {
        path
    } else {
        value.strip_prefix("ssh://git@github.com/")?
    };
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    parse_full_name(path)
}

fn parse_full_name(value: &str) -> Option<RepositorySlug> {
    let (owner, name) = value.split_once('/')?;
    if name.contains('/')
        || validate_slug_component(owner, value).is_err()
        || validate_slug_component(name, value).is_err()
    {
        return None;
    }
    Some(RepositorySlug::new(owner, name))
}

fn validate_slug_component(component: &str, selector: &str) -> Result<(), GithubError> {
    let valid = !component.is_empty()
        && component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(GithubError::InvalidSelector(selector.to_owned()))
    }
}

fn validate_sha(value: &str) -> Result<(), String> {
    if value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!("invalid GitHub commit SHA {value:?}"))
    }
}

/// Fetches the PR head and its base branch into namespaced refs.
///
/// Returns the base branch tip that was actually fetched. GitHub's `base.sha` is
/// pinned when a pull request is created or synchronized and does not follow the
/// base branch, so it is recorded as provenance but is never used as the
/// comparison base. The reviewable change is the merge base of the current base
/// branch tip and the head, which is also what GitHub's own "Files changed" view
/// shows.
///
/// Only the head is verified against the API response: a head that no longer
/// matches means the PR was pushed to while it was loading, which would silently
/// change what is under review.
fn fetch_snapshot(
    repository: &Path,
    remote: &str,
    metadata: &PullRequestMetadata,
) -> Result<String, GithubError> {
    let namespace = format!(
        "refs/zreview/github/{}/{}/pull/{}",
        ref_component(&metadata.repository.owner),
        ref_component(&metadata.repository.name),
        metadata.number
    );
    let base_destination = format!("{namespace}/base");
    let head_destination = format!("{namespace}/head");
    let refspecs = [
        format!("+refs/heads/{}:{base_destination}", metadata.base_ref),
        format!("+refs/pull/{}/head:{head_destination}", metadata.number),
    ];
    git::fetch_refspecs(repository, remote, &refspecs)?;

    let head_sha = git::resolve_commit(repository, &head_destination)?;
    if head_sha != metadata.head_sha.as_ref() {
        return Err(GithubError::HeadMoved {
            expected: metadata.head_sha.to_string(),
            actual: head_sha,
        });
    }

    Ok(git::resolve_commit(repository, &base_destination)?)
}

fn ref_component(value: &str) -> String {
    let mut component = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if component.is_empty() {
        component.push('_');
    }
    component
}

fn successful_output(output: Output) -> Result<Output, GithubError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(GithubError::Command {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    use super::*;
    use tempfile::TempDir;

    const BASE_SHA: &str = "1111111111111111111111111111111111111111";
    const HEAD_SHA: &str = "2222222222222222222222222222222222222222";

    #[test]
    fn parses_pull_request_urls_and_remote_formats() {
        let locator =
            parse_pull_request_url("https://github.com/acme/widgets/pull/42/files#diff-example")
                .unwrap();
        assert_eq!(locator.repository, RepositorySlug::new("acme", "widgets"));
        assert_eq!(locator.number, 42);

        for remote in [
            "git@github.com:acme/widgets.git",
            "https://github.com/acme/widgets.git",
            "ssh://git@github.com/acme/widgets.git",
        ] {
            assert_eq!(
                parse_remote_url(remote),
                Some(RepositorySlug::new("acme", "widgets")),
            );
        }
        assert!(parse_pull_request_url("http://github.com/acme/widgets/pull/42").is_err());
        assert_eq!(
            parse_remote_url("https://example.com/acme/widgets.git"),
            None
        );
    }

    #[test]
    fn fetches_and_normalizes_metadata_with_a_fake_gh() {
        let directory = TempDir::new().unwrap();
        let gh = directory.path().join("gh");
        write_fake_gh(&gh, BASE_SHA, HEAD_SHA);
        let client = GithubClient::new(&gh);
        let locator = PullRequestLocator {
            repository: RepositorySlug::new("acme", "widgets"),
            number: 42,
        };

        let metadata = client.fetch_metadata(directory.path(), &locator).unwrap();

        assert_eq!(metadata.number, 42);
        assert_eq!(metadata.title, "Improve the review flow");
        assert_eq!(metadata.base_ref, "main");
        assert_eq!(metadata.head_ref, "feature/review");
        assert_eq!(metadata.base_sha.as_ref(), BASE_SHA);
        assert_eq!(metadata.head_sha.as_ref(), HEAD_SHA);
    }

    #[test]
    fn loads_a_pr_into_namespaced_refs_and_a_comparison() {
        let source = TempDir::new().unwrap();
        git(source.path(), ["init", "--quiet", "--initial-branch=main"]);
        git(source.path(), ["config", "user.name", "ZReview Test"]);
        git(
            source.path(),
            ["config", "user.email", "zreview@example.invalid"],
        );
        fs::write(source.path().join("review.txt"), "base\n").unwrap();
        git(source.path(), ["add", "."]);
        git(source.path(), ["commit", "--quiet", "-m", "base"]);
        let base_sha = git_output(source.path(), ["rev-parse", "HEAD"]);
        git(
            source.path(),
            ["checkout", "--quiet", "-b", "feature/review"],
        );
        fs::write(source.path().join("review.txt"), "base\nhead\n").unwrap();
        git(source.path(), ["add", "."]);
        git(source.path(), ["commit", "--quiet", "-m", "head"]);
        let head_sha = git_output(source.path(), ["rev-parse", "HEAD"]);
        git(
            source.path(),
            ["update-ref", "refs/pull/42/head", &head_sha],
        );

        let target = TempDir::new().unwrap();
        git(target.path(), ["init", "--quiet"]);
        let github_url = "https://github.com/acme/widgets.git";
        git(target.path(), ["remote", "add", "origin", github_url]);
        git(
            target.path(),
            [
                "config",
                &format!("url.{}.insteadOf", source.path().display()),
                github_url,
            ],
        );

        let gh = target.path().join("fake-gh");
        write_fake_gh(&gh, &base_sha, &head_sha);
        let result = GithubClient::new(&gh)
            .load_pull_request(target.path(), &PullRequestSelector::Number(42))
            .unwrap();

        assert_eq!(result.metadata.title, "Improve the review flow");
        assert_eq!(result.comparison.files.len(), 1);
        assert_eq!(result.comparison.files[0].path.as_ref(), "review.txt");
        assert_eq!(
            git_output(
                target.path(),
                ["rev-parse", "refs/zreview/github/acme/widgets/pull/42/head"],
            ),
            head_sha,
        );
    }

    /// GitHub pins `base.sha` when a PR is created or synchronized, so on any
    /// active repository it lags the real base branch tip. Loading must still
    /// succeed and must review only the PR's own commits.
    #[test]
    fn loads_a_pr_whose_recorded_base_sha_has_been_left_behind() {
        let source = TempDir::new().unwrap();
        git(source.path(), ["init", "--quiet", "--initial-branch=main"]);
        git(source.path(), ["config", "user.name", "ZReview Test"]);
        git(
            source.path(),
            ["config", "user.email", "zreview@example.invalid"],
        );
        fs::write(source.path().join("shared.txt"), "fork point\n").unwrap();
        git(source.path(), ["add", "."]);
        git(source.path(), ["commit", "--quiet", "-m", "fork point"]);
        // What GitHub will report as base.sha, captured before main moves on.
        let recorded_base_sha = git_output(source.path(), ["rev-parse", "HEAD"]);

        git(
            source.path(),
            ["checkout", "--quiet", "-b", "feature/review"],
        );
        fs::write(source.path().join("feature.txt"), "feature work\n").unwrap();
        git(source.path(), ["add", "."]);
        git(source.path(), ["commit", "--quiet", "-m", "feature"]);
        let head_sha = git_output(source.path(), ["rev-parse", "HEAD"]);
        git(
            source.path(),
            ["update-ref", "refs/pull/42/head", &head_sha],
        );

        git(source.path(), ["checkout", "--quiet", "main"]);
        fs::write(source.path().join("unrelated.txt"), "base moved on\n").unwrap();
        git(source.path(), ["add", "."]);
        git(source.path(), ["commit", "--quiet", "-m", "base moves"]);
        let base_tip_sha = git_output(source.path(), ["rev-parse", "HEAD"]);
        assert_ne!(recorded_base_sha, base_tip_sha);

        let target = TempDir::new().unwrap();
        git(target.path(), ["init", "--quiet"]);
        let github_url = "https://github.com/acme/widgets.git";
        git(target.path(), ["remote", "add", "origin", github_url]);
        git(
            target.path(),
            [
                "config",
                &format!("url.{}.insteadOf", source.path().display()),
                github_url,
            ],
        );

        let gh = target.path().join("fake-gh");
        write_fake_gh(&gh, &recorded_base_sha, &head_sha);
        let result = GithubClient::new(&gh)
            .load_pull_request(target.path(), &PullRequestSelector::Number(42))
            .unwrap();

        // Provenance keeps GitHub's value; the comparison uses the real tip.
        assert_eq!(result.metadata.base_sha.as_ref(), recorded_base_sha);
        assert_eq!(result.comparison.base_sha.as_ref(), base_tip_sha);
        assert_eq!(result.comparison.diff_base_sha.as_ref(), recorded_base_sha);

        // The base branch's own commit is not part of the review.
        let paths = result
            .comparison
            .files
            .iter()
            .map(|file| file.path.to_string())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["feature.txt"]);
    }

    #[test]
    fn rejects_a_head_that_moved_while_loading() {
        let source = TempDir::new().unwrap();
        git(source.path(), ["init", "--quiet", "--initial-branch=main"]);
        git(source.path(), ["config", "user.name", "ZReview Test"]);
        git(
            source.path(),
            ["config", "user.email", "zreview@example.invalid"],
        );
        fs::write(source.path().join("review.txt"), "base\n").unwrap();
        git(source.path(), ["add", "."]);
        git(source.path(), ["commit", "--quiet", "-m", "base"]);
        let base_sha = git_output(source.path(), ["rev-parse", "HEAD"]);
        fs::write(source.path().join("review.txt"), "base\nhead\n").unwrap();
        git(source.path(), ["add", "."]);
        git(source.path(), ["commit", "--quiet", "-m", "head"]);
        let actual_head = git_output(source.path(), ["rev-parse", "HEAD"]);
        git(
            source.path(),
            ["update-ref", "refs/pull/42/head", &actual_head],
        );

        let target = TempDir::new().unwrap();
        git(target.path(), ["init", "--quiet"]);
        let github_url = "https://github.com/acme/widgets.git";
        git(target.path(), ["remote", "add", "origin", github_url]);
        git(
            target.path(),
            [
                "config",
                &format!("url.{}.insteadOf", source.path().display()),
                github_url,
            ],
        );

        // The API reports a head that the remote no longer has.
        let gh = target.path().join("fake-gh");
        write_fake_gh(&gh, &base_sha, HEAD_SHA);
        let error = GithubClient::new(&gh)
            .load_pull_request(target.path(), &PullRequestSelector::Number(42))
            .unwrap_err();

        assert!(
            matches!(&error, GithubError::HeadMoved { expected, actual }
                if expected == HEAD_SHA && *actual == actual_head),
            "unexpected error: {error}"
        );
    }

    fn write_fake_gh(path: &Path, base_sha: &str, head_sha: &str) {
        let body = format!(
            r#"#!/bin/sh
if [ "$1" != "api" ] || [ "$2" != "--method" ] || [ "$3" != "GET" ] || [ "$4" != "repos/acme/widgets/pulls/42" ]; then
  echo "unexpected gh arguments: $*" >&2
  exit 64
fi
cat <<'JSON'
{{
  "number": 42,
  "title": "Improve the review flow",
  "html_url": "https://github.com/acme/widgets/pull/42",
  "state": "open",
  "draft": false,
  "base": {{"ref": "main", "sha": "{base_sha}", "repo": {{"full_name": "acme/widgets"}}}},
  "head": {{"ref": "feature/review", "sha": "{head_sha}", "repo": {{"full_name": "contributor/widgets"}}}}
}}
JSON
"#,
        );
        fs::write(path, body).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn git<I, S>(repository: &Path, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let status = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn git_output<I, S>(repository: &Path, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}
