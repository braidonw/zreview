use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::Arc,
    time::Duration,
};

use domain::{DiffSide, LoadStage, ReviewComment, ReviewSubmission};
use git::{ComparisonDiff, ComparisonMode, GitRemote};
use serde::Deserialize;
use thiserror::Error;

mod home;

pub use home::{
    DEFAULT_GRAPHQL_TIMEOUT, HomeFetch, HomePullRequest, HomeRepository, HomeSearch,
    OpinionatedReview, REPOSITORIES_PER_BATCH, RateLimit, ReviewDecision, ReviewState,
    ReviewThread, StatusCheckState,
};

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

/// A local clone resolved to its worktree root and the GitHub repository it
/// points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedClone {
    pub root: PathBuf,
    pub slug: RepositorySlug,
}

/// Why a local clone is not something Home can list pull requests for.
#[derive(Debug, Error)]
pub enum CloneError {
    #[error("the folder no longer exists")]
    Missing,

    #[error("not a Git repository")]
    NotAGitRepository,

    #[error("no GitHub remote")]
    NoGithubRemote,

    #[error("could not read the repository: {detail}")]
    Unreadable { detail: String },
}

/// Resolves a local clone to its worktree root and the GitHub repository its
/// remotes name.
///
/// The root, rather than the path handed in, is what identifies a clone, so two
/// paths inside one checkout resolve to the same entry.
///
/// # Errors
///
/// Returns [`CloneError`] when the path is gone, is not a Git repository, has no
/// remote that parses to a GitHub slug, or cannot be read.
pub fn resolve_clone(path: &Path) -> Result<ResolvedClone, CloneError> {
    // The error kind separates a clone that has been moved away from one this
    // process is not allowed to look at, which are different problems.
    if let Err(error) = std::fs::metadata(path) {
        return Err(match error.kind() {
            std::io::ErrorKind::NotFound => CloneError::Missing,
            _ => CloneError::Unreadable {
                detail: error.to_string(),
            },
        });
    }
    let root = git::repository_root(path).map_err(|error| classify_git_failure(&error))?;
    let remotes = git::remotes(&root).map_err(|error| classify_git_failure(&error))?;
    let slug = preferred_remote_repository(&remotes).ok_or(CloneError::NoGithubRemote)?;
    Ok(ResolvedClone { root, slug })
}

/// Separates Git saying the folder is not a checkout from Git failing for any
/// other reason.
///
/// Only Git's own wording earns the verdict. A folder Git could not enter, or a
/// Git that could not run at all, is a fault reported with the text it gave.
fn classify_git_failure(error: &git::GitError) -> CloneError {
    match error {
        git::GitError::Command { stderr, .. } if is_not_a_repository(stderr) => {
            CloneError::NotAGitRepository
        }
        git::GitError::Command { .. }
        | git::GitError::Execute { .. }
        | git::GitError::NonUtf8 { .. }
        | git::GitError::InvalidObjectId { .. }
        | git::GitError::UnsupportedStatus(_)
        | git::GitError::InvalidPath(_)
        | git::GitError::InvalidPatch { .. } => CloneError::Unreadable {
            detail: error.to_string(),
        },
    }
}

/// Git's own wording for a path that is not inside a checkout.
fn is_not_a_repository(stderr: &str) -> bool {
    stderr.to_lowercase().contains("not a git repository")
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

    #[error("the GitHub CLI (gh) was not found")]
    GhMissing,

    #[error("failed to execute gh in {repository}: {source}")]
    Execute {
        repository: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// As [`GithubError::Execute`], for the calls that name repositories by slug
    /// and so run in no repository of their own.
    #[error("failed to run gh: {source}")]
    Spawn {
        #[source]
        source: std::io::Error,
    },

    #[error("GitHub rejected the request as unauthenticated: {detail}")]
    Unauthenticated { detail: String },

    #[error("GitHub refused the request: {detail}")]
    Forbidden { detail: String },

    #[error("GitHub has no such repository or pull request: {detail}")]
    NotFound { detail: String },

    #[error("the GitHub API rate limit is exhausted: {detail}")]
    RateLimited { detail: String },

    #[error("GitHub rejected the request as invalid: {detail}")]
    Validation { detail: String },

    #[error("GitHub returned a server error (HTTP {status}): {detail}")]
    ServerError { status: u16, detail: String },

    #[error("could not reach GitHub: {detail}")]
    Network { detail: String },

    #[error("gh api failed with status {status}: {stderr}")]
    Command { status: i32, stderr: String },

    #[error("gh did not respond within {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("GitHub returned more than {pages} pages of {subject}")]
    PagingLimit { pages: usize, subject: &'static str },

    #[error("GitHub returned invalid pull request JSON: {0}")]
    InvalidResponse(#[from] serde_json::Error),

    /// A GraphQL response that parsed but cannot be trusted, such as a page that
    /// claims a successor it gives no cursor for.
    #[error("GitHub returned an unusable response: {detail}")]
    InvalidGraphResponse { detail: String },

    #[error("GitHub review comment {id} is unusable: {message}")]
    InvalidComment { id: u64, message: String },

    #[error("GitHub returned PR #{actual} when #{expected} was requested")]
    UnexpectedPullRequest { expected: u64, actual: u64 },

    #[error("GitHub returned repository {actual}, expected {expected}")]
    UnexpectedRepository { expected: String, actual: String },

    #[error(
        "the pull request was updated while it was loading (expected head {expected}, fetched {actual})"
    )]
    HeadMoved { expected: String, actual: String },

    #[error(transparent)]
    Git(#[from] git::GitError),
}

impl GithubError {
    /// The next thing the reviewer can actually do about this failure.
    ///
    /// Returns `None` when there is no honest advice to give; inventing one would
    /// be worse than showing the error alone.
    #[must_use]
    pub const fn remediation(&self) -> Option<&'static str> {
        match self {
            Self::GhMissing => {
                Some("Install the GitHub CLI from https://cli.github.com, then retry.")
            }
            Self::Unauthenticated { .. } => {
                Some("Run `gh auth login`, then reopen the pull request.")
            }
            Self::Forbidden { .. } => Some(
                "Check that your GitHub token has `repo` scope and that any required SSO authorization is active.",
            ),
            Self::NotFound { .. } => Some(
                "Check the pull request number, and that the authenticated account can see the repository.",
            ),
            Self::RateLimited { .. } => {
                Some("Wait for the GitHub rate limit to reset, then retry.")
            }
            Self::Network { .. } | Self::Timeout { .. } => {
                Some("Check your network connection and https://githubstatus.com, then retry.")
            }
            Self::ServerError { .. } => Some("Check https://githubstatus.com, then retry."),
            Self::HeadMoved { .. } => {
                Some("Reload the pull request to review its new head commit.")
            }
            Self::NoGithubRemote | Self::NoMatchingRemote(_) => Some(
                "Open a clone whose remote matches the pull request, or pass the full pull request URL.",
            ),
            Self::InvalidSelector(_) => Some(
                "Pass a pull request number, or a full https://github.com/owner/repo/pull/N URL.",
            ),
            Self::Command { .. }
            | Self::Execute { .. }
            | Self::InvalidGraphResponse { .. }
            | Self::PagingLimit { .. }
            | Self::Spawn { .. }
            | Self::Validation { .. }
            | Self::InvalidResponse(_)
            | Self::InvalidComment { .. }
            | Self::UnexpectedPullRequest { .. }
            | Self::UnexpectedRepository { .. }
            | Self::Git(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GithubClient {
    gh_executable: PathBuf,
    graphql_timeout: Duration,
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
            graphql_timeout: DEFAULT_GRAPHQL_TIMEOUT,
        }
    }

    /// Sets how long one GraphQL call may run before `gh` is killed.
    #[must_use]
    pub const fn with_graphql_timeout(mut self, timeout: Duration) -> Self {
        self.graphql_timeout = timeout;
        self
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
        self.load_pull_request_reporting(repository, selector, &|_| {})
    }

    /// Loads a pull request, reporting each stage as it starts.
    ///
    /// Loading runs several subprocesses and can take a while on a large pull
    /// request, so the caller is told what is happening rather than leaving the
    /// reviewer behind an unexplained wait.
    ///
    /// # Errors
    ///
    /// As [`GithubClient::load_pull_request`].
    pub fn load_pull_request_reporting(
        &self,
        repository: &Path,
        selector: &PullRequestSelector,
        report: &dyn Fn(LoadStage),
    ) -> Result<PullRequestDiff, GithubError> {
        let root = git::repository_root(repository)?;
        let remotes = git::remotes(&root)?;
        let locator = resolve_selector(selector, &remotes)?;

        // Checked before the first API call so an unauthenticated reviewer gets
        // "run gh auth login" instead of a 401 from whatever happened to run
        // first.
        report(LoadStage::CheckingAuthentication);
        self.check_authentication(&root)?;

        report(LoadStage::ReadingPullRequest);
        let metadata = self.fetch_metadata(&root, &locator)?;
        let remote = select_remote(&remotes, &metadata.repository)
            .ok_or_else(|| GithubError::NoMatchingRemote(metadata.repository.full_name()))?;

        report(LoadStage::FetchingObjects);
        let base_tip_sha = fetch_snapshot(&root, remote, &metadata)?;

        report(LoadStage::BuildingDiff);
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

    /// Confirms `gh` exists and holds usable credentials.
    ///
    /// # Errors
    ///
    /// Returns [`GithubError::GhMissing`] when the CLI is not installed and
    /// [`GithubError::Unauthenticated`] when it is not logged in.
    pub fn check_authentication(&self, repository: &Path) -> Result<(), GithubError> {
        let output = Command::new(&self.gh_executable)
            .current_dir(repository)
            .args(["auth", "status"])
            .env("GH_PROMPT_DISABLED", "1")
            .output()
            .map_err(|source| execution_error(repository, source))?;

        if output.status.success() {
            return Ok(());
        }

        // `gh auth status` reports the problem on stdout, unlike `gh api`.
        let detail = [&output.stdout, &output.stderr]
            .into_iter()
            .map(|stream| String::from_utf8_lossy(stream).trim().to_owned())
            .find(|text| !text.is_empty())
            .unwrap_or_else(|| "gh auth status reported no active account".to_owned());
        Err(GithubError::Unauthenticated { detail })
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
        let stdout = self.api(repository, &[], &endpoint)?;
        let response: ApiPullRequest = serde_json::from_slice(&stdout)?;
        normalize_metadata(response, locator)
    }

    /// Fetches every published inline review comment on a pull request.
    ///
    /// Comments are returned in the order GitHub published them, replies
    /// included. Grouping them into threads and deciding where each one belongs
    /// is [`domain::PlacedComments`]'s job, because that depends on the snapshot
    /// rather than on the response.
    ///
    /// # Errors
    ///
    /// Returns [`GithubError`] when `gh` fails, a page is malformed, or a comment
    /// names a diff side that is not `LEFT` or `RIGHT`.
    pub fn fetch_review_comments(
        &self,
        repository: &Path,
        locator: &PullRequestLocator,
    ) -> Result<Vec<ReviewComment>, GithubError> {
        let endpoint = format!(
            "repos/{}/{}/pulls/{}/comments?per_page=100",
            locator.repository.owner, locator.repository.name, locator.number
        );
        let stdout = self.api(repository, &["--paginate"], &endpoint)?;
        parse_review_comments(&stdout)
    }

    /// Posts a review to a pull request.
    ///
    /// The head is re-read first and the submission refused if it moved: a review
    /// pinned to a commit that is no longer the head would attach comments to code
    /// the author has already replaced. Nothing local is touched here, so a
    /// refusal or a failure leaves every draft where it was — the caller decides
    /// what to forget, and only after this returns successfully.
    ///
    /// # Errors
    ///
    /// Returns [`GithubError::HeadMoved`] when the pull request advanced past the
    /// snapshot, or the usual categories when `gh` fails.
    pub fn submit_review(
        &self,
        repository: &Path,
        locator: &PullRequestLocator,
        submission: &ReviewSubmission,
    ) -> Result<SubmittedReview, GithubError> {
        let current = self.fetch_metadata(repository, locator)?;
        if current.head_sha != submission.head_sha {
            return Err(GithubError::HeadMoved {
                expected: submission.head_sha.to_string(),
                actual: current.head_sha.to_string(),
            });
        }

        let endpoint = format!(
            "repos/{}/{}/pulls/{}/reviews",
            locator.repository.owner, locator.repository.name, locator.number
        );
        let payload = review_payload(submission);
        let stdout = self.api_post(repository, &endpoint, &payload)?;
        let response: ApiReview = serde_json::from_slice(&stdout)?;

        Ok(SubmittedReview {
            id: response.id,
            state: response.state,
            url: response.html_url,
        })
    }

    /// Runs one `gh api` POST, sending the body on stdin.
    ///
    /// The payload goes over stdin rather than as arguments so a comment body
    /// cannot end up in a process listing, and so its size is not bounded by the
    /// argument limit.
    fn api_post(
        &self,
        repository: &Path,
        endpoint: &str,
        payload: &serde_json::Value,
    ) -> Result<Vec<u8>, GithubError> {
        let body = serde_json::to_vec(payload)?;
        let mut child = Command::new(&self.gh_executable)
            .current_dir(repository)
            .args([
                "api",
                "--method",
                "POST",
                endpoint,
                "--input",
                "-",
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "X-GitHub-Api-Version: 2022-11-28",
            ])
            .env("GH_PROMPT_DISABLED", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| execution_error(repository, source))?;

        child
            .stdin
            .take()
            .ok_or_else(|| {
                execution_error(
                    repository,
                    std::io::Error::other("gh stdin was not available"),
                )
            })?
            .write_all(&body)
            .map_err(|source| execution_error(repository, source))?;

        let output = child
            .wait_with_output()
            .map_err(|source| execution_error(repository, source))?;
        Ok(successful_output(output)?.stdout)
    }

    /// Runs one `gh api` call and returns its stdout.
    ///
    /// Arguments are passed as an array so no shell is involved, and the REST API
    /// version is pinned so a server-side default change cannot alter the
    /// response shape underneath the parsers.
    fn api(
        &self,
        repository: &Path,
        extra: &[&str],
        endpoint: &str,
    ) -> Result<Vec<u8>, GithubError> {
        let mut args = vec!["api", "--method", "GET"];
        args.extend_from_slice(extra);
        args.extend_from_slice(&[
            endpoint,
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "X-GitHub-Api-Version: 2022-11-28",
        ]);

        let output = Command::new(&self.gh_executable)
            .current_dir(repository)
            .args(&args)
            .env("GH_PROMPT_DISABLED", "1")
            .output()
            .map_err(|source| execution_error(repository, source))?;
        Ok(successful_output(output)?.stdout)
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

/// A review the forge accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmittedReview {
    pub id: u64,
    pub state: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct ApiReview {
    id: u64,
    state: String,
    html_url: String,
}

/// Builds the exact request body GitHub's create-review endpoint expects.
///
/// `commit_id` pins the review, so the forge itself rejects it if the head moved
/// between the check above and this call. `event` is always present: omitting it
/// would leave a pending review on the forge, which PLAN rules out.
///
/// `line` and `side` are used rather than the closing-down `position` parameter.
fn review_payload(submission: &ReviewSubmission) -> serde_json::Value {
    let comments = submission
        .comments
        .iter()
        .map(|comment| {
            let mut json = serde_json::json!({
                "path": comment.path.as_ref(),
                "line": comment.line,
                "side": comment.side.github_value(),
                "body": comment.body,
            });
            // GitHub requires both range fields together. The start side always
            // matches, because a range across revisions is never built.
            if let Some(start_line) = comment.start_line {
                json["start_line"] = start_line.into();
                json["start_side"] = comment.side.github_value().into();
            }
            json
        })
        .collect::<Vec<_>>();

    let mut payload = serde_json::json!({
        "commit_id": submission.head_sha.as_ref(),
        "event": submission.event.github_value(),
    });
    // An approval may legitimately have no body, and sending an empty string is
    // not the same as sending nothing.
    if !submission.body.is_empty() {
        payload["body"] = serde_json::Value::String(submission.body.clone());
    }
    if !comments.is_empty() {
        payload["comments"] = serde_json::Value::Array(comments);
    }
    payload
}

#[derive(Debug, Deserialize)]
struct ApiReviewComment {
    id: u64,
    body: String,
    path: String,
    /// Absent when GitHub considers the comment outdated or it is file-level.
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    start_line: Option<u32>,
    /// GitHub omits this for comments on the head revision.
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    in_reply_to_id: Option<u64>,
    /// `"line"` or `"file"`.
    #[serde(default)]
    subject_type: Option<String>,
    /// Null once the author's account is deleted.
    #[serde(default)]
    user: Option<ApiUser>,
    created_at: String,
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct ApiUser {
    login: String,
}

/// GitHub's placeholder for a deleted account, matching what its own UI shows.
const DELETED_USER: &str = "ghost";

/// Parses the output of a paginated `gh api` call over review comments.
///
/// `gh --paginate` merges array pages into one array in current versions but has
/// emitted one array per page, so both framings are accepted rather than
/// depending on the installed CLI's behaviour.
fn parse_review_comments(stdout: &[u8]) -> Result<Vec<ReviewComment>, GithubError> {
    let mut comments = Vec::new();
    for page in serde_json::Deserializer::from_slice(stdout).into_iter::<Vec<ApiReviewComment>>() {
        for comment in page? {
            comments.push(normalize_review_comment(comment)?);
        }
    }
    Ok(comments)
}

fn normalize_review_comment(comment: ApiReviewComment) -> Result<ReviewComment, GithubError> {
    // An unrecognized side would silently place the comment against the wrong
    // revision, so it is reported rather than guessed.
    let side = match comment.side.as_deref() {
        None => DiffSide::Right,
        Some(value) => DiffSide::from_github(value).ok_or_else(|| GithubError::InvalidComment {
            id: comment.id,
            message: format!("unknown diff side {value:?}"),
        })?,
    };

    Ok(ReviewComment {
        id: comment.id,
        author: comment
            .user
            .map_or_else(|| DELETED_USER.into(), |user| user.login.into()),
        body: comment.body.into(),
        path: comment.path.into(),
        side,
        line: comment.line,
        start_line: comment.start_line,
        in_reply_to_id: comment.in_reply_to_id,
        is_file_level: comment.subject_type.as_deref() == Some("file"),
        created_at: comment.created_at.into(),
        url: comment.html_url.into(),
    })
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

/// Reads `owner/name` back into a slug, refusing anything else.
///
/// Public so a caller holding a formatted slug can ask for it in the shape the
/// fetches take, without a parse of its own that could disagree with this one.
#[must_use]
pub fn parse_full_name(value: &str) -> Option<RepositorySlug> {
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
    let valid = !component.is_empty() && component.bytes().all(is_slug_byte);
    if valid {
        Ok(())
    } else {
        Err(GithubError::InvalidSelector(selector.to_owned()))
    }
}

/// The bytes GitHub allows in an owner or a repository name.
const fn is_slug_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
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
        Err(classify_failure(&output))
    }
}

/// Distinguishes "gh is not installed" from other spawn failures, because the
/// remediation is completely different.
fn execution_error(repository: &Path, source: std::io::Error) -> GithubError {
    if source.kind() == std::io::ErrorKind::NotFound {
        GithubError::GhMissing
    } else {
        GithubError::Execute {
            repository: repository.to_path_buf(),
            source,
        }
    }
}

/// Sorts a failed `gh api` call into a category the caller can act on.
///
/// `gh` exits 1 for every HTTP error and reports the status in stderr as
/// `gh: <message> (HTTP <status>)`, so the status is recovered from there rather
/// than from the exit code. Anything unrecognized stays a generic command
/// failure instead of being forced into a category that might be wrong.
fn classify_failure(output: &Output) -> GithubError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let status = output.status.code().unwrap_or(-1);

    match http_status(&stderr) {
        Some(401) => GithubError::Unauthenticated { detail: stderr },
        // GitHub reports an exhausted rate limit as 403 with an explanatory body,
        // and 429 only sometimes, so both are checked.
        Some(403 | 429) if mentions_rate_limit(&stderr) => {
            GithubError::RateLimited { detail: stderr }
        }
        Some(429) => GithubError::RateLimited { detail: stderr },
        Some(403) => GithubError::Forbidden { detail: stderr },
        Some(404) => GithubError::NotFound { detail: stderr },
        Some(422) => GithubError::Validation { detail: stderr },
        Some(server) if server >= 500 => GithubError::ServerError {
            status: server,
            detail: stderr,
        },
        _ if is_network_failure(&stderr) => GithubError::Network { detail: stderr },
        _ => GithubError::Command { status, stderr },
    }
}

/// Extracts the status from `gh`'s `(HTTP <status>)` suffix.
fn http_status(stderr: &str) -> Option<u16> {
    let after = stderr.split("(HTTP ").nth(1)?;
    let digits = after.split(')').next()?;
    digits.trim().parse().ok()
}

fn mentions_rate_limit(stderr: &str) -> bool {
    let lowered = stderr.to_lowercase();
    lowered.contains("rate limit") || lowered.contains("secondary rate")
}

fn is_network_failure(stderr: &str) -> bool {
    let lowered = stderr.to_lowercase();
    lowered.contains("error connecting to")
        || lowered.contains("internet connection")
        || lowered.contains("dial tcp")
        || lowered.contains("no such host")
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    use super::*;
    use tempfile::TempDir;

    const BASE_SHA: &str = "1111111111111111111111111111111111111111";
    const HEAD_SHA: &str = "2222222222222222222222222222222222222222";

    /// Exit status alone cannot tell these apart: `gh` returns 1 for every HTTP
    /// error, so the category comes from the reported status.
    #[test]
    fn classifies_gh_api_failures_by_reported_http_status() {
        /// A stderr sample and the category it must land in.
        type Case = (&'static str, fn(&GithubError) -> bool);

        let cases: [Case; 8] = [
            ("gh: Bad credentials (HTTP 401)", |error| {
                matches!(error, GithubError::Unauthenticated { .. })
            }),
            ("gh: Resource not accessible (HTTP 403)", |error| {
                matches!(error, GithubError::Forbidden { .. })
            }),
            ("gh: API rate limit exceeded (HTTP 403)", |error| {
                matches!(error, GithubError::RateLimited { .. })
            }),
            (
                "gh: You have exceeded a secondary rate limit (HTTP 429)",
                |error| matches!(error, GithubError::RateLimited { .. }),
            ),
            ("gh: Not Found (HTTP 404)", |error| {
                matches!(error, GithubError::NotFound { .. })
            }),
            ("gh: Validation Failed (HTTP 422)", |error| {
                matches!(error, GithubError::Validation { .. })
            }),
            ("gh: Server Error (HTTP 503)", |error| {
                matches!(error, GithubError::ServerError { status: 503, .. })
            }),
            (
                "error connecting to api.github.com\ncheck your internet connection",
                |error| matches!(error, GithubError::Network { .. }),
            ),
        ];

        for (stderr, matches_category) in cases {
            let error = classify_failure(&failed_output(stderr));
            assert!(
                matches_category(&error),
                "{stderr:?} was classified as {error:?}",
            );
            // A rejected payload is a defect in this application rather than
            // something a reviewer can act on, so it is the one category with no
            // honest advice to offer.
            let expects_remediation = !matches!(error, GithubError::Validation { .. });
            assert_eq!(
                error.remediation().is_some(),
                expects_remediation,
                "{stderr:?} remediation did not match expectation",
            );
            // The reported text survives classification in every case.
            let shown = error.to_string();
            assert!(
                shown.contains("HTTP") || shown.contains("connecting"),
                "detail was lost for {stderr:?}: {shown}",
            );
        }
    }

    #[test]
    fn an_unrecognized_failure_stays_a_generic_command_error() {
        // Better a plain report than a category that might be wrong.
        let error = classify_failure(&failed_output("gh: something entirely new"));

        assert!(matches!(error, GithubError::Command { status: 1, .. }));
        assert_eq!(error.remediation(), None);
    }

    #[test]
    fn extracts_the_http_status_from_gh_stderr() {
        assert_eq!(http_status("gh: Not Found (HTTP 404)"), Some(404));
        assert_eq!(
            http_status("gh: Server Error (HTTP 500)"),
            None.or(Some(500))
        );
        assert_eq!(http_status("no status here"), None);
        assert_eq!(http_status("gh: broken (HTTP notanumber)"), None);
    }

    #[test]
    fn a_missing_gh_executable_is_reported_as_such() {
        let directory = TempDir::new().unwrap();
        let client = GithubClient::new(directory.path().join("gh-does-not-exist"));

        let error = client.check_authentication(directory.path()).unwrap_err();

        assert!(matches!(error, GithubError::GhMissing));
        assert!(
            error.remediation().unwrap().contains("cli.github.com"),
            "should point at installing gh",
        );
    }

    #[test]
    fn an_unauthenticated_gh_is_caught_before_any_api_call() {
        let directory = TempDir::new().unwrap();
        let gh = directory.path().join("gh");
        // Mirrors `gh auth status`, which reports on stdout and exits 1.
        write_executable(
            &gh,
            "#!/bin/sh\nif [ \"$1\" = \"auth\" ]; then\n  echo 'The token in GH_TOKEN is invalid.'\n  exit 1\nfi\necho 'api should not have been reached' >&2\nexit 99\n",
        );

        let error = GithubClient::new(&gh)
            .check_authentication(directory.path())
            .unwrap_err();

        assert!(
            matches!(&error, GithubError::Unauthenticated { detail }
                if detail.contains("GH_TOKEN is invalid")),
            "unexpected error: {error}",
        );
        assert!(error.remediation().unwrap().contains("gh auth login"));
    }

    #[test]
    fn loading_reports_each_stage_in_order() {
        let directory = TempDir::new().unwrap();
        git(directory.path(), ["init", "--quiet"]);
        let gh = directory.path().join("gh");
        write_fake_gh(&gh, BASE_SHA, HEAD_SHA);

        let stages = std::cell::RefCell::new(Vec::new());
        // This clone has no matching remote, so loading fails partway — but only
        // after reporting the stages it did reach.
        let error = GithubClient::new(&gh)
            .load_pull_request_reporting(
                directory.path(),
                &PullRequestSelector::Url("https://github.com/acme/widgets/pull/42".to_owned()),
                &|stage| stages.borrow_mut().push(stage),
            )
            .unwrap_err();
        assert!(
            matches!(error, GithubError::NoMatchingRemote(_)),
            "unexpected error: {error}",
        );

        assert_eq!(
            stages.into_inner(),
            [
                LoadStage::CheckingAuthentication,
                LoadStage::ReadingPullRequest
            ],
            "authentication is checked before anything is requested",
        );
    }

    fn failed_output(stderr: &str) -> Output {
        Output {
            status: exit_status_one(),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn exit_status_one() -> std::process::ExitStatus {
        Command::new("sh").args(["-c", "exit 1"]).status().unwrap()
    }

    fn submission(event: domain::ReviewEvent, body: &str) -> ReviewSubmission {
        ReviewSubmission {
            head_sha: HEAD_SHA.into(),
            event,
            body: body.to_owned(),
            comments: vec![
                domain::SubmittableComment {
                    path: "src/review.rs".into(),
                    side: DiffSide::Right,
                    line: 11,
                    start_line: None,
                    body: "needs a test".to_owned(),
                },
                domain::SubmittableComment {
                    path: "src/review.rs".into(),
                    side: DiffSide::Left,
                    line: 6,
                    start_line: None,
                    body: "why was this removed?".to_owned(),
                },
            ],
            excluded: Vec::new(),
        }
    }

    #[test]
    fn the_payload_pins_the_head_and_uses_line_and_side() {
        let payload = review_payload(&submission(domain::ReviewEvent::Comment, "Two notes."));

        assert_eq!(payload["commit_id"], HEAD_SHA);
        assert_eq!(payload["event"], "COMMENT");
        assert_eq!(payload["body"], "Two notes.");

        let comments = payload["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0]["path"], "src/review.rs");
        assert_eq!(comments[0]["line"], 11);
        assert_eq!(comments[0]["side"], "RIGHT");
        assert_eq!(comments[0]["body"], "needs a test");
        assert_eq!(comments[1]["side"], "LEFT");

        // `position` is the parameter GitHub is retiring; it must not appear.
        assert!(
            comments
                .iter()
                .all(|comment| comment.get("position").is_none())
        );
        // No range on these, so neither range field is sent.
        assert!(
            comments
                .iter()
                .all(|comment| comment.get("start_line").is_none()
                    && comment.get("start_side").is_none())
        );
    }

    /// GitHub requires `start_line` and `start_side` together, and the start side
    /// always matches because a range across revisions is never built.
    #[test]
    fn a_range_comment_sends_both_range_fields() {
        let mut ranged = submission(domain::ReviewEvent::Comment, "One note.");
        ranged.comments.truncate(1);
        ranged.comments[0].start_line = Some(8);

        let payload = review_payload(&ranged);
        let comment = &payload["comments"][0];

        assert_eq!(comment["start_line"], 8);
        assert_eq!(comment["line"], 11);
        assert_eq!(comment["side"], "RIGHT");
        assert_eq!(comment["start_side"], "RIGHT");
    }

    /// An approval with no body must send no `body` key at all: an empty string is
    /// not the same as absent, and GitHub only permits the latter.
    #[test]
    fn an_approval_without_a_body_omits_the_key() {
        let payload = review_payload(&submission(domain::ReviewEvent::Approve, ""));

        assert_eq!(payload["event"], "APPROVE");
        assert!(payload.get("body").is_none());
    }

    #[test]
    fn a_review_with_no_inline_comments_omits_the_comments_key() {
        let mut only_summary = submission(domain::ReviewEvent::Comment, "Just a note.");
        only_summary.comments.clear();
        let payload = review_payload(&only_summary);

        assert!(payload.get("comments").is_none());
        assert_eq!(payload["body"], "Just a note.");
    }

    #[test]
    fn submitting_sends_the_payload_on_stdin_and_reads_the_response() {
        let directory = TempDir::new().unwrap();
        let gh = directory.path().join("gh");
        let captured = directory.path().join("captured.json");
        write_fake_submit_gh(&gh, &captured, HEAD_SHA);
        let locator = PullRequestLocator {
            repository: RepositorySlug::new("acme", "widgets"),
            number: 42,
        };

        let submitted = GithubClient::new(&gh)
            .submit_review(
                directory.path(),
                &locator,
                &submission(domain::ReviewEvent::RequestChanges, "Two notes."),
            )
            .unwrap();

        assert_eq!(submitted.id, 909);
        assert_eq!(submitted.state, "CHANGES_REQUESTED");
        assert!(submitted.url.contains("acme/widgets"));

        // The body reached gh over stdin, not as an argument.
        let sent: serde_json::Value =
            serde_json::from_slice(&fs::read(&captured).unwrap()).unwrap();
        assert_eq!(sent["event"], "REQUEST_CHANGES");
        assert_eq!(sent["commit_id"], HEAD_SHA);
        assert_eq!(sent["comments"].as_array().unwrap().len(), 2);
    }

    /// The guard that stops a review being attached to code the author has already
    /// replaced.
    #[test]
    fn submitting_is_refused_when_the_head_has_moved() {
        let directory = TempDir::new().unwrap();
        let gh = directory.path().join("gh");
        let captured = directory.path().join("captured.json");
        // The pull request now reports a different head than the snapshot's.
        write_fake_submit_gh(&gh, &captured, BASE_SHA);
        let locator = PullRequestLocator {
            repository: RepositorySlug::new("acme", "widgets"),
            number: 42,
        };

        let error = GithubClient::new(&gh)
            .submit_review(
                directory.path(),
                &locator,
                &submission(domain::ReviewEvent::Comment, "Two notes."),
            )
            .unwrap_err();

        assert!(
            matches!(&error, GithubError::HeadMoved { expected, actual }
                if expected == HEAD_SHA && actual == BASE_SHA),
            "unexpected error: {error}",
        );
        assert!(error.remediation().unwrap().contains("Reload"));
        // Nothing was posted.
        assert!(!captured.exists(), "the review must not have been sent");
    }

    /// Answers the metadata read with `head_sha`, then records any POST body.
    fn write_fake_submit_gh(path: &Path, captured: &Path, head_sha: &str) {
        let body = format!(
            r#"#!/bin/sh
if [ "$1" = "auth" ]; then exit 0; fi
if [ "$3" = "GET" ]; then
cat <<'JSON'
{{
  "number": 42,
  "title": "Improve the review flow",
  "html_url": "https://github.com/acme/widgets/pull/42",
  "state": "open",
  "draft": false,
  "base": {{"ref": "main", "sha": "{BASE_SHA}", "repo": {{"full_name": "acme/widgets"}}}},
  "head": {{"ref": "feature/review", "sha": "{head_sha}", "repo": {{"full_name": "acme/widgets"}}}}
}}
JSON
  exit 0
fi
if [ "$3" != "POST" ] || [ "$4" != "repos/acme/widgets/pulls/42/reviews" ]; then
  echo "unexpected gh arguments: $*" >&2
  exit 64
fi
cat > "{captured}"
cat <<'JSON'
{{"id": 909, "state": "CHANGES_REQUESTED", "html_url": "https://github.com/acme/widgets/pull/42#pullrequestreview-909"}}
JSON
"#,
            captured = captured.display(),
        );
        write_executable(path, &body);
    }

    /// A clone with `origin` pointing at GitHub, which is what Home configures.
    fn clone_with_remote(url: Option<&str>) -> TempDir {
        let directory = TempDir::new().unwrap();
        git(directory.path(), ["init", "--quiet"]);
        if let Some(url) = url {
            git(directory.path(), ["remote", "add", "origin", url]);
        }
        directory
    }

    #[test]
    fn a_clone_resolves_to_its_worktree_root_and_github_slug() {
        let directory = clone_with_remote(Some("git@github.com:acme/widgets.git"));
        let nested = directory.path().join("crates/review");
        fs::create_dir_all(&nested).unwrap();

        let resolved = resolve_clone(&nested).unwrap();

        assert_eq!(resolved.slug, RepositorySlug::new("acme", "widgets"));
        assert_eq!(
            resolved.root,
            directory.path().canonicalize().unwrap(),
            "a path inside the clone should resolve to the clone's root",
        );
    }

    #[test]
    fn a_clone_with_no_github_remote_is_refused() {
        let directory = clone_with_remote(Some("https://example.com/acme/widgets.git"));

        let error = resolve_clone(directory.path()).unwrap_err();

        assert!(matches!(error, CloneError::NoGithubRemote), "got {error}");
        assert_eq!(error.to_string(), "no GitHub remote");
    }

    #[test]
    fn a_clone_with_no_remotes_at_all_is_refused() {
        let directory = clone_with_remote(None);

        let error = resolve_clone(directory.path()).unwrap_err();

        assert!(matches!(error, CloneError::NoGithubRemote), "got {error}");
    }

    #[test]
    fn a_folder_that_is_not_a_git_repository_is_refused() {
        let directory = TempDir::new().unwrap();

        let error = resolve_clone(directory.path()).unwrap_err();

        assert!(
            matches!(error, CloneError::NotAGitRepository),
            "got {error}",
        );
        assert_eq!(error.to_string(), "not a Git repository");
    }

    #[test]
    fn a_folder_that_no_longer_exists_is_refused_as_missing() {
        let directory = TempDir::new().unwrap();

        let error = resolve_clone(&directory.path().join("moved-away")).unwrap_err();

        assert!(matches!(error, CloneError::Missing), "got {error}");
        assert_eq!(error.to_string(), "the folder no longer exists");
    }

    /// A folder the process may not look inside, so `git` cannot enter it and
    /// the failure is a fault rather than a verdict on the folder.
    #[cfg(unix)]
    #[test]
    fn a_folder_git_cannot_enter_is_refused_as_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let locked = directory.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let error = resolve_clone(&locked).unwrap_err();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            matches!(&error, CloneError::Unreadable { detail } if detail.contains("Permission denied")),
            "got {error}",
        );
    }

    /// A folder whose parent cannot be read at all, which is not the same as one
    /// that has been moved away.
    #[cfg(unix)]
    #[test]
    fn a_folder_whose_metadata_cannot_be_read_is_refused_as_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let locked = directory.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let error = resolve_clone(&locked.join("clone")).unwrap_err();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            matches!(error, CloneError::Unreadable { .. }),
            "a folder that cannot be looked at is not a folder that is gone: {error}",
        );
    }

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

    /// Two pages emitted as concatenated arrays, the framing `gh --paginate`
    /// produces when a jq filter is in play, covering every field shape the
    /// mapper has to handle.
    const COMMENT_PAGES: &str = r#"[
  {
    "id": 10,
    "body": "This branch is not covered.",
    "path": "src/review.rs",
    "line": 11,
    "start_line": null,
    "side": "RIGHT",
    "in_reply_to_id": null,
    "subject_type": "line",
    "user": {"login": "reviewer"},
    "created_at": "2026-07-20T10:00:00Z",
    "html_url": "https://github.com/acme/widgets/pull/42#discussion_r10"
  },
  {
    "id": 11,
    "body": "Agreed, adding a test.",
    "path": "src/review.rs",
    "line": 11,
    "side": "RIGHT",
    "in_reply_to_id": 10,
    "subject_type": "line",
    "user": null,
    "created_at": "2026-07-20T11:00:00Z",
    "html_url": "https://github.com/acme/widgets/pull/42#discussion_r11"
  }
]
[
  {
    "id": 12,
    "body": "Why was this removed?",
    "path": "src/review.rs",
    "line": 10,
    "start_line": 8,
    "side": "LEFT",
    "in_reply_to_id": null,
    "subject_type": "line",
    "user": {"login": "maintainer"},
    "created_at": "2026-07-21T09:00:00Z",
    "html_url": "https://github.com/acme/widgets/pull/42#discussion_r12"
  },
  {
    "id": 13,
    "body": "This whole file needs an owner.",
    "path": "src/review.rs",
    "line": null,
    "side": "RIGHT",
    "in_reply_to_id": null,
    "subject_type": "file",
    "user": {"login": "maintainer"},
    "created_at": "2026-07-21T09:30:00Z",
    "html_url": "https://github.com/acme/widgets/pull/42#discussion_r13"
  },
  {
    "id": 14,
    "body": "Stale note on code that has since changed.",
    "path": "src/review.rs",
    "line": null,
    "side": "RIGHT",
    "in_reply_to_id": null,
    "subject_type": "line",
    "user": {"login": "reviewer"},
    "created_at": "2026-07-21T10:00:00Z",
    "html_url": "https://github.com/acme/widgets/pull/42#discussion_r14"
  }
]"#;

    #[test]
    fn maps_paginated_review_comments() {
        let comments = parse_review_comments(COMMENT_PAGES.as_bytes()).unwrap();

        assert_eq!(
            comments
                .iter()
                .map(|comment| comment.id)
                .collect::<Vec<_>>(),
            [10, 11, 12, 13, 14],
            "both pages should be read, in order",
        );

        let first = &comments[0];
        assert_eq!(first.author.as_ref(), "reviewer");
        assert_eq!(first.side, DiffSide::Right);
        assert_eq!(first.line, Some(11));
        assert!(!first.is_file_level);
        assert!(!first.is_multiline());

        // A deleted account falls back to GitHub's own placeholder.
        assert_eq!(comments[1].author.as_ref(), DELETED_USER);
        assert_eq!(comments[1].in_reply_to_id, Some(10));

        let left = &comments[2];
        assert_eq!(left.side, DiffSide::Left);
        assert_eq!(left.start_line, Some(8));
        assert!(left.is_multiline());

        assert!(comments[3].is_file_level);
        assert_eq!(comments[3].line, None);

        // No line and not file-level: outdated.
        assert!(!comments[4].is_file_level);
        assert_eq!(comments[4].line, None);
    }

    /// Guards the mapper against the real response shape, which carries 26 fields
    /// per comment where the mapper reads nine.
    #[test]
    fn maps_a_captured_github_response() {
        let captured = include_bytes!("../tests/fixtures/review-comments.json");
        let comments = parse_review_comments(captured).unwrap();

        assert_eq!(comments.len(), 3);
        for comment in &comments {
            assert_eq!(comment.author.as_ref(), "Copilot");
            assert_eq!(comment.path.as_ref(), "crates/globset/src/lib.rs");
            assert_eq!(comment.side, DiffSide::Right);
            assert!(!comment.is_file_level);
            assert!(comment.in_reply_to_id.is_none());
            // Every comment in this response spans a range.
            assert!(comment.is_multiline(), "expected a multi-line range");
            assert!(!comment.body.is_empty());
            assert!(comment.url.starts_with("https://github.com/"));
        }
        assert_eq!(comments[0].line, Some(753));
        assert_eq!(comments[0].start_line, Some(749));
    }

    #[test]
    fn reads_pages_that_gh_merged_into_one_array() {
        // Current gh versions concatenate array pages into a single array.
        let merged = COMMENT_PAGES.replace("]\n[", ",");
        let comments = parse_review_comments(merged.as_bytes()).unwrap();

        assert_eq!(comments.len(), 5);
        assert_eq!(comments[4].id, 14);
    }

    #[test]
    fn an_absent_side_defaults_to_the_head_revision() {
        let body = br#"[{"id":1,"body":"b","path":"p","line":3,
            "created_at":"t","html_url":"u","user":{"login":"a"}}]"#;
        let comments = parse_review_comments(body).unwrap();

        assert_eq!(comments[0].side, DiffSide::Right);
    }

    #[test]
    fn an_unknown_side_is_reported_rather_than_guessed() {
        let body = br#"[{"id":7,"body":"b","path":"p","line":3,"side":"MIDDLE",
            "created_at":"t","html_url":"u","user":{"login":"a"}}]"#;
        let error = parse_review_comments(body).unwrap_err();

        assert!(
            matches!(&error, GithubError::InvalidComment { id: 7, message }
                if message.contains("MIDDLE")),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn an_empty_comment_response_is_not_an_error() {
        assert!(parse_review_comments(b"[]").unwrap().is_empty());
        assert!(parse_review_comments(b"").unwrap().is_empty());
        assert!(parse_review_comments(b"[]\n[]\n").unwrap().is_empty());
    }

    #[test]
    fn fetches_review_comments_through_gh() {
        let directory = TempDir::new().unwrap();
        let gh = directory.path().join("gh");
        write_fake_comment_gh(&gh);
        let locator = PullRequestLocator {
            repository: RepositorySlug::new("acme", "widgets"),
            number: 42,
        };

        let comments = GithubClient::new(&gh)
            .fetch_review_comments(directory.path(), &locator)
            .unwrap();

        assert_eq!(comments.len(), 5);
        assert_eq!(comments[0].path.as_ref(), "src/review.rs");
    }

    /// Asserts the exact argument array, including `--paginate` and the pinned
    /// API version, then emits two concatenated pages.
    fn write_fake_comment_gh(path: &Path) {
        let body = format!(
            r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then exit 0; fi
expected="api --method GET --paginate repos/acme/widgets/pulls/42/comments?per_page=100 -H Accept: application/vnd.github+json -H X-GitHub-Api-Version: 2022-11-28"
if [ "$*" != "$expected" ]; then
  echo "unexpected gh arguments:" >&2
  echo "  got:      $*" >&2
  echo "  expected: $expected" >&2
  exit 64
fi
cat <<'JSON'
{COMMENT_PAGES}
JSON
"#
        );
        write_executable(path, &body);
    }

    fn write_fake_gh(path: &Path, base_sha: &str, head_sha: &str) {
        let body = format!(
            r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  echo "Logged in to github.com as zreview-test"
  exit 0
fi
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
        write_executable(path, &body);
    }

    fn write_executable(path: &Path, body: &str) {
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
