//! Turning a request to review something into a loaded [`ReviewSession`].
//!
//! Loading runs Git and `gh` subprocesses, so it must not happen on a UI thread.
//! This crate is deliberately free of any UI dependency: it takes a request,
//! reports stages as it goes, and returns either a session or a failure the
//! reviewer can act on. Whoever calls it decides which thread it runs on.

use std::path::{Path, PathBuf};

use domain::{DiffFile, FileStatus, LoadStage, ReviewSession, SessionFailure, SessionSource};
use git::{ComparisonMode, GitError};
use github::{GithubClient, GithubError, PullRequestLocator, PullRequestSelector};

/// How many files the generated fixture contains, and how long its first file is.
const DEMO_FILES: usize = 12;
const DEMO_STRESS_LINES: usize = 100_000;

/// What the reviewer asked to open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionRequest {
    /// The generated fixture, which needs no repository.
    Demo,
    LocalComparison {
        repository: PathBuf,
        base: String,
        head: String,
    },
    PullRequest {
        repository: PathBuf,
        selector: PullRequestSelector,
    },
}

impl SessionRequest {
    /// A short description of what is being opened, for the loading screen.
    ///
    /// Shown before anything is known about the target, so it can only describe
    /// the request itself.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::Demo => "the generated fixture".to_owned(),
            Self::LocalComparison { base, head, .. } => format!("{base}…{head}"),
            Self::PullRequest { selector, .. } => match selector {
                PullRequestSelector::Number(number) => format!("pull request #{number}"),
                PullRequestSelector::Url(url) => url.clone(),
            },
        }
    }
}

/// Loads a session, reporting each stage as it begins.
///
/// `report` is called from whatever thread this runs on, so a caller crossing
/// threads is responsible for getting the update where it needs to go.
///
/// # Errors
///
/// Returns a [`SessionFailure`] carrying a summary, the underlying detail, and
/// remediation when the failure is one the reviewer can act on.
pub fn load(
    request: &SessionRequest,
    report: &dyn Fn(LoadStage),
) -> Result<ReviewSession, SessionFailure> {
    report(LoadStage::Starting);
    match request {
        SessionRequest::Demo => Ok(load_demo()),
        SessionRequest::LocalComparison {
            repository,
            base,
            head,
        } => load_local(repository, base, head, report),
        SessionRequest::PullRequest {
            repository,
            selector,
        } => load_pull_request(repository, selector, report),
    }
}

fn load_local(
    repository: &Path,
    base: &str,
    head: &str,
    report: &dyn Fn(LoadStage),
) -> Result<ReviewSession, SessionFailure> {
    report(LoadStage::BuildingDiff);
    let comparison = git::load_comparison(repository, base, head, ComparisonMode::MergeBase)
        .map_err(|error| describe_git_failure(&error, repository))?;
    let source = SessionSource::LocalComparison {
        repository_root: comparison.repository_root,
        base_sha: comparison.base_sha,
        diff_base_sha: comparison.diff_base_sha,
        head_sha: comparison.head_sha,
    };

    ReviewSession::new(source, comparison.files).map_err(|_| {
        SessionFailure::new(format!("{base}…{head} contains no changed files"))
            .with_remediation("Choose revisions that differ, or pass an explicit head revision.")
    })
}

fn load_pull_request(
    repository: &Path,
    selector: &PullRequestSelector,
    report: &dyn Fn(LoadStage),
) -> Result<ReviewSession, SessionFailure> {
    let client = GithubClient::default();
    let pull_request = client
        .load_pull_request_reporting(repository, selector, report)
        .map_err(|error| describe_github_failure(&error, repository))?;

    let metadata = pull_request.metadata;
    let comparison = pull_request.comparison;
    let number = metadata.number;
    let locator = PullRequestLocator {
        repository: metadata.repository.clone(),
        number,
    };
    let source = SessionSource::GitHubPullRequest {
        repository_root: comparison.repository_root,
        owner: metadata.repository.owner.into(),
        repository: metadata.repository.name.into(),
        number,
        title: metadata.title.into(),
        url: metadata.url.into(),
        base_ref: metadata.base_ref.into(),
        head_ref: metadata.head_ref.into(),
        base_sha: comparison.base_sha,
        recorded_base_sha: metadata.base_sha,
        diff_base_sha: comparison.diff_base_sha,
        head_sha: metadata.head_sha,
    };

    let mut session = ReviewSession::new(source, comparison.files).map_err(|_| {
        SessionFailure::new(format!("pull request #{number} contains no changed files"))
    })?;

    // A pull request is worth reading even if its conversation could not be
    // fetched, so this is reported through the session rather than failing it.
    report(LoadStage::LoadingConversations);
    match client.fetch_review_comments(repository, &locator) {
        Ok(comments) => {
            session.set_review_comments(comments);
        }
        Err(error) => session.set_comment_load_failure(describe_github_failure(&error, repository)),
    }

    Ok(session)
}

fn load_demo() -> ReviewSession {
    let files = (0..DEMO_FILES)
        .map(|index| {
            let mut file = DiffFile::demo(if index == 0 {
                DEMO_STRESS_LINES
            } else {
                200 + index * 25
            });
            file.path = format!("src/review_fixture_{index:02}.rs").into();
            file.status = match index % 4 {
                0 => FileStatus::Modified,
                1 => FileStatus::Added,
                2 => FileStatus::Deleted,
                _ => FileStatus::Renamed,
            };
            file
        })
        .collect::<Vec<_>>();

    ReviewSession::new(SessionSource::Demo, files.into())
        .expect("the generated fixture always contains files")
}

/// Describes a Git failure, naming the likely cause where Git's own message is
/// too low-level to act on.
fn describe_git_failure(error: &GitError, repository: &Path) -> SessionFailure {
    let failure = SessionFailure::from_error("Could not build the comparison", error);
    match error {
        GitError::Execute { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            SessionFailure::from_error("Git was not found", error)
                .with_remediation("Install the Xcode command-line tools, then retry.")
        }
        GitError::Command { operation, .. } if *operation == "rev-parse" => failure
            .with_remediation(format!(
                "Check that {} is inside a Git repository and that both revisions exist.",
                repository.display()
            )),
        GitError::InvalidObjectId { .. } => {
            failure.with_remediation("Check that both revisions name existing commits.")
        }
        _ => failure,
    }
}

fn describe_github_failure(error: &GithubError, repository: &Path) -> SessionFailure {
    let summary = match error {
        GithubError::GhMissing => "The GitHub CLI is not installed",
        GithubError::Unauthenticated { .. } => "GitHub is not authenticated",
        GithubError::Forbidden { .. } => "GitHub refused access",
        GithubError::NotFound { .. } => "That pull request could not be found",
        GithubError::RateLimited { .. } => "GitHub's rate limit is exhausted",
        GithubError::Network { .. } => "GitHub could not be reached",
        GithubError::ServerError { .. } => "GitHub returned a server error",
        GithubError::HeadMoved { .. } => "The pull request changed while it was loading",
        GithubError::NoGithubRemote | GithubError::NoMatchingRemote(_) => {
            "This repository does not match the pull request"
        }
        GithubError::InvalidSelector(_) => "That is not a valid pull request",
        GithubError::Git(git_error) => return describe_git_failure(git_error, repository),
        _ => "Could not load the pull request",
    };

    SessionFailure::from_error(summary, error).with_optional_remediation(error.remediation())
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, ffi::OsStr, fs, process::Command};

    use super::*;
    use tempfile::TempDir;

    #[test]
    fn the_demo_request_loads_without_a_repository() {
        let session = load(&SessionRequest::Demo, &|_| {}).unwrap();

        assert_eq!(session.files().len(), DEMO_FILES);
        assert_eq!(session.files()[0].line_count(), DEMO_STRESS_LINES);
        // No head commit, so nothing can be anchored.
        assert!(session.anchors().is_none());
    }

    #[test]
    fn a_local_comparison_reports_its_stage_and_loads() {
        let repository = temporary_repository();
        let stages = RefCell::new(Vec::new());

        let session = load(
            &SessionRequest::LocalComparison {
                repository: repository.path().to_path_buf(),
                base: "main".to_owned(),
                head: "feature".to_owned(),
            },
            &|stage| stages.borrow_mut().push(stage),
        )
        .unwrap();

        assert_eq!(
            stages.into_inner(),
            [LoadStage::Starting, LoadStage::BuildingDiff],
        );
        let paths = session
            .files()
            .iter()
            .map(|file| file.path.to_string())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["feature.txt"]);
        assert!(session.anchors().is_some());
    }

    #[test]
    fn a_directory_outside_a_repository_explains_itself() {
        let directory = TempDir::new().unwrap();

        let failure = load(
            &SessionRequest::LocalComparison {
                repository: directory.path().to_path_buf(),
                base: "main".to_owned(),
                head: "HEAD".to_owned(),
            },
            &|_| {},
        )
        .unwrap_err();

        assert_eq!(failure.summary, "Could not build the comparison");
        assert!(failure.detail.is_some(), "the Git error is preserved");
        assert!(
            failure
                .remediation
                .as_deref()
                .is_some_and(|text| text.contains("Git repository")),
            "unexpected remediation: {:?}",
            failure.remediation,
        );
    }

    #[test]
    fn comparing_a_revision_with_itself_is_an_actionable_failure() {
        let repository = temporary_repository();

        let failure = load(
            &SessionRequest::LocalComparison {
                repository: repository.path().to_path_buf(),
                base: "main".to_owned(),
                head: "main".to_owned(),
            },
            &|_| {},
        )
        .unwrap_err();

        assert!(failure.summary.contains("no changed files"));
        assert!(failure.remediation.is_some());
    }

    #[test]
    fn an_unresolvable_revision_says_so() {
        let repository = temporary_repository();

        let failure = load(
            &SessionRequest::LocalComparison {
                repository: repository.path().to_path_buf(),
                base: "no-such-branch".to_owned(),
                head: "main".to_owned(),
            },
            &|_| {},
        )
        .unwrap_err();

        assert!(failure.remediation.is_some(), "should be actionable");
    }

    #[test]
    fn requests_describe_themselves_for_the_loading_screen() {
        assert_eq!(SessionRequest::Demo.description(), "the generated fixture");
        assert_eq!(
            SessionRequest::LocalComparison {
                repository: PathBuf::from("/tmp/repository"),
                base: "main".to_owned(),
                head: "HEAD".to_owned(),
            }
            .description(),
            "main…HEAD",
        );
        assert_eq!(
            SessionRequest::PullRequest {
                repository: PathBuf::from("/tmp/repository"),
                selector: PullRequestSelector::Number(42),
            }
            .description(),
            "pull request #42",
        );
    }

    #[test]
    fn github_failures_keep_their_remediation() {
        let failure = describe_github_failure(
            &GithubError::Unauthenticated {
                detail: "The token in GH_TOKEN is invalid.".to_owned(),
            },
            Path::new("/tmp/repository"),
        );

        assert_eq!(failure.summary, "GitHub is not authenticated");
        assert!(failure.detail.unwrap().contains("GH_TOKEN"));
        assert!(failure.remediation.unwrap().contains("gh auth login"));
    }

    /// A Git error surfacing through the GitHub client should be described as the
    /// Git problem it is, not as a generic pull request failure.
    #[test]
    fn a_git_failure_inside_github_loading_is_described_as_git() {
        let failure = describe_github_failure(
            &GithubError::Git(GitError::InvalidObjectId {
                revision: "HEAD".to_owned(),
                value: "not-a-sha".to_owned(),
            }),
            Path::new("/tmp/repository"),
        );

        assert_eq!(failure.summary, "Could not build the comparison");
        assert!(failure.remediation.unwrap().contains("existing commits"));
    }

    fn temporary_repository() -> TempDir {
        let repository = TempDir::new().unwrap();
        let path = repository.path();
        git(path, ["init", "--quiet", "--initial-branch=main"]);
        git(path, ["config", "user.name", "ZReview Test"]);
        git(path, ["config", "user.email", "zreview@example.invalid"]);
        fs::write(path.join("shared.txt"), "fork point\n").unwrap();
        git(path, ["add", "."]);
        git(path, ["commit", "--quiet", "-m", "fork point"]);
        git(path, ["checkout", "--quiet", "-b", "feature"]);
        fs::write(path.join("feature.txt"), "feature work\n").unwrap();
        git(path, ["add", "."]);
        git(path, ["commit", "--quiet", "-m", "feature"]);
        git(path, ["checkout", "--quiet", "main"]);
        repository
    }

    fn git<I, S>(repository: &Path, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let status = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
