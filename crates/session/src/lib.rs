//! Turning a request to review something into a loaded [`ReviewSession`].
//!
//! Loading runs Git and `gh` subprocesses, so it must not happen on a UI thread.
//! This crate is deliberately free of any UI dependency: it takes a request,
//! reports stages as it goes, and returns either a session or a failure the
//! reviewer can act on. Whoever calls it decides which thread it runs on.

use std::path::{Path, PathBuf};

use std::sync::Arc;

use domain::{
    DiffAnchor, DiffFile, DraftSink, FileStatus, LoadStage, LoadedSession, ReviewSession,
    ReviewSubmission, ReviewSubmitter, SessionFailure, SessionSource, SubmissionOutcome,
};
use git::{ComparisonMode, GitError};
use github::{GithubClient, GithubError, PullRequestLocator, PullRequestSelector};
use store::{DraftStore, DraftWriter, StoreError};

mod review_run;

pub use review_run::{ReviewRun, run as run_review};

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
    drafts: &DraftStorage,
    report: &dyn Fn(LoadStage),
) -> Result<LoadedSession, SessionFailure> {
    report(LoadStage::Starting);
    // Only a pull request has somewhere to submit to, so only that branch produces
    // a submitter.
    let (mut session, submitter) = match request {
        SessionRequest::Demo => (load_demo(), None),
        SessionRequest::LocalComparison {
            repository,
            base,
            head,
        } => (load_local(repository, base, head, report)?, None),
        SessionRequest::PullRequest {
            repository,
            selector,
        } => {
            let (session, locator) = load_pull_request(repository, selector, report)?;
            let submitter: Arc<dyn ReviewSubmitter> = Arc::new(GithubSubmitter {
                client: GithubClient::default(),
                repository: repository.clone(),
                locator,
            });
            (session, Some(submitter))
        }
    };

    let draft_sink = attach_draft_storage(&mut session, drafts);
    Ok(LoadedSession {
        session,
        draft_sink,
        submitter,
    })
}

/// Where drafts are persisted.
///
/// Passed in rather than resolved internally so a test never touches the
/// reviewer's real review data, and so relocating it later needs no new plumbing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum DraftStorage {
    /// The review data directory in the user's Application Support.
    #[default]
    Default,
    /// A specific database file.
    At(PathBuf),
    /// Keep drafts in memory only; nothing is written.
    Disabled,
}

impl DraftStorage {
    fn path(&self) -> Option<Result<PathBuf, StoreError>> {
        match self {
            Self::Default => Some(store::default_database_path()),
            Self::At(path) => Some(Ok(path.clone())),
            Self::Disabled => None,
        }
    }
}

/// Restores saved drafts into the session and returns where new ones are written.
///
/// Storage failing does not fail the load: a reviewer can still read a diff
/// without persistence. It is recorded as a warning instead, because typing into
/// something that is not saving is exactly the situation they must be told about.
fn attach_draft_storage(
    session: &mut ReviewSession,
    drafts: &DraftStorage,
) -> Option<Box<dyn DraftSink>> {
    let scope = session.source().draft_scope()?;
    let path = match drafts.path()? {
        Ok(path) => path,
        Err(error) => {
            session.push_warning(draft_storage_warning(&error));
            return None;
        }
    };

    // One connection to read the existing drafts, and a second for the writer
    // thread, so a connection is never shared across threads.
    let reader = match DraftStore::open(&path) {
        Ok(reader) => reader,
        Err(error) => {
            session.push_warning(draft_storage_warning(&error));
            return None;
        }
    };
    match reader.load(&scope) {
        Ok(saved) => restore_saved_drafts(session, saved),
        Err(error) => session.push_warning(draft_storage_warning(&error)),
    }

    match DraftStore::open(&path) {
        Ok(writer) => Some(Box::new(DraftWriter::spawn(writer, scope))),
        Err(error) => {
            session.push_warning(draft_storage_warning(&error));
            None
        }
    }
}

/// Sends confirmed reviews to GitHub.
///
/// Holds the locator rather than deriving it at submission time, so a review is
/// always posted to the pull request it was read from.
struct GithubSubmitter {
    client: GithubClient,
    repository: PathBuf,
    locator: PullRequestLocator,
}

impl ReviewSubmitter for GithubSubmitter {
    fn submit(&self, submission: &ReviewSubmission) -> Result<SubmissionOutcome, SessionFailure> {
        let submitted = self
            .client
            .submit_review(&self.repository, &self.locator, submission)
            .map_err(|error| describe_submission_failure(&error, &self.repository))?;

        Ok(SubmissionOutcome {
            state: submitted.state,
            url: submitted.url,
            comment_count: submission.comments.len(),
        })
    }
}

/// Describes a failed submission, leading with the fact that nothing was lost.
///
/// A reviewer whose submission just failed needs to know their words are still
/// there before they need to know why it failed.
fn describe_submission_failure(error: &GithubError, repository: &Path) -> SessionFailure {
    let summary = match error {
        GithubError::HeadMoved { .. } => "The pull request moved on before this could be submitted",
        GithubError::Validation { .. } => "GitHub rejected the review",
        _ => return describe_github_failure(error, repository),
    };

    SessionFailure::from_error(summary, error).with_remediation(match error {
        GithubError::HeadMoved { .. } => {
            "Your drafts are unchanged. Reopen the pull request to re-anchor them against the new head, then submit again."
        }
        _ => "Your drafts are unchanged, so nothing was lost and you can submit again.",
    })
}

fn restore_saved_drafts(session: &mut ReviewSession, saved: Vec<(DiffAnchor, String)>) {
    let restored = session.restore_drafts(saved);
    if restored.stale == 0 {
        return;
    }

    session.push_warning(
        SessionFailure::new(format!(
            "{} saved draft{} no longer match this diff",
            restored.stale,
            if restored.stale == 1 { "" } else { "s" },
        ))
        .with_remediation(
            "They are kept and shown, but must be moved to a line in the current diff before they can be submitted.",
        ),
    );
}

fn draft_storage_warning(error: &StoreError) -> SessionFailure {
    SessionFailure::from_error("Drafts are not being saved", error).with_remediation(
        "Review can continue, but anything written will be lost when ZReview closes.",
    )
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
) -> Result<(ReviewSession, PullRequestLocator), SessionFailure> {
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
        Err(error) => session.push_warning(describe_github_failure(&error, repository)),
    }

    Ok((session, locator))
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
        let session = load(&SessionRequest::Demo, &DraftStorage::Disabled, &|_| {})
            .unwrap()
            .session;

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
            &DraftStorage::Disabled,
            &|stage| stages.borrow_mut().push(stage),
        )
        .unwrap()
        .session;

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
            &DraftStorage::Disabled,
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
            &DraftStorage::Disabled,
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
            &DraftStorage::Disabled,
            &|_| {},
        )
        .unwrap_err();

        assert!(failure.remediation.is_some(), "should be actionable");
    }

    fn local_request(repository: &TempDir) -> SessionRequest {
        SessionRequest::LocalComparison {
            repository: repository.path().to_path_buf(),
            base: "main".to_owned(),
            head: "feature".to_owned(),
        }
    }

    /// The whole point of persistence: what was typed comes back after the
    /// process that typed it is gone.
    #[test]
    fn a_draft_survives_reopening_the_session() {
        let repository = temporary_repository();
        let data = TempDir::new().unwrap();
        let storage = DraftStorage::At(data.path().join("review-data.sqlite3"));
        let request = local_request(&repository);

        {
            let mut loaded = load(&request, &storage, &|_| {}).unwrap();
            let sink = loaded.draft_sink.as_ref().expect("drafts should persist");
            assert!(loaded.session.set_draft(0, 0, "worth keeping"));
            let anchor = loaded.session.anchor_for(0, 0).unwrap();
            sink.save(&anchor, "worth keeping");
            assert_eq!(sink.failure(), None);
            // Dropping joins the writer thread, so the write has landed.
        }

        let reopened = load(&request, &storage, &|_| {}).unwrap().session;

        assert_eq!(reopened.drafts().len(), 1);
        let draft = reopened.draft_at(0, 0).expect("it should re-anchor");
        assert_eq!(draft.body, "worth keeping");
        assert!(!draft.is_stale);
        assert!(reopened.warnings().is_empty(), "nothing to warn about");
    }

    #[test]
    fn a_discarded_draft_does_not_come_back() {
        let repository = temporary_repository();
        let data = TempDir::new().unwrap();
        let storage = DraftStorage::At(data.path().join("review-data.sqlite3"));
        let request = local_request(&repository);

        {
            let loaded = load(&request, &storage, &|_| {}).unwrap();
            let sink = loaded.draft_sink.as_ref().unwrap();
            let anchor = loaded.session.anchor_for(0, 0).unwrap();
            sink.save(&anchor, "second thoughts");
            sink.discard(&anchor);
        }

        assert!(
            load(&request, &storage, &|_| {})
                .unwrap()
                .session
                .drafts()
                .is_empty()
        );
    }

    /// A draft written against a head that is no longer current is kept and the
    /// reviewer is told, rather than being silently dropped or silently moved.
    #[test]
    fn a_draft_from_another_head_is_restored_as_stale_with_a_warning() {
        let repository = temporary_repository();
        let data = TempDir::new().unwrap();
        let path = data.path().join("review-data.sqlite3");
        let storage = DraftStorage::At(path.clone());
        let request = local_request(&repository);

        // Write directly, as an earlier head would have.
        {
            let scope = load(&request, &DraftStorage::Disabled, &|_| {})
                .unwrap()
                .session
                .source()
                .draft_scope()
                .unwrap();
            let store = DraftStore::open(&path).unwrap();
            store
                .upsert(
                    &scope,
                    &DiffAnchor {
                        path: "feature.txt".into(),
                        side: domain::DiffSide::Right,
                        line: 1,
                        start_line: None,
                        head_sha: "0".repeat(40).into(),
                    },
                    "written before the branch moved",
                )
                .unwrap();
        }

        let reopened = load(&request, &storage, &|_| {}).unwrap().session;

        assert_eq!(reopened.drafts().stale_count(), 1);
        assert_eq!(
            reopened.drafts().stale().next().unwrap().body,
            "written before the branch moved",
        );
        let warning = reopened
            .warnings()
            .iter()
            .find(|warning| warning.summary.contains("no longer match"))
            .expect("the reviewer should be told");
        assert!(warning.remediation.is_some());
    }

    #[test]
    fn disabled_storage_leaves_drafts_in_memory_only() {
        let repository = temporary_repository();
        let loaded = load(
            &local_request(&repository),
            &DraftStorage::Disabled,
            &|_| {},
        )
        .unwrap();

        assert!(loaded.draft_sink.is_none());
        assert!(loaded.session.warnings().is_empty(), "not a failure");
    }

    /// A generated fixture has no identity to store drafts under, so it gets no
    /// sink and no complaint about it.
    #[test]
    fn the_demo_persists_nothing() {
        let loaded = load(&SessionRequest::Demo, &DraftStorage::Default, &|_| {}).unwrap();

        assert!(loaded.draft_sink.is_none());
        assert!(loaded.session.warnings().is_empty());
    }

    #[test]
    fn unusable_storage_warns_but_still_opens_the_session() {
        let repository = temporary_repository();
        // A path whose parent cannot be created, because it is a file.
        let file = TempDir::new().unwrap();
        let blocker = file.path().join("not-a-directory");
        std::fs::write(&blocker, "").unwrap();
        let storage = DraftStorage::At(blocker.join("review-data.sqlite3"));

        let loaded = load(&local_request(&repository), &storage, &|_| {}).unwrap();

        assert!(loaded.draft_sink.is_none(), "nowhere to write");
        let warning = loaded
            .session
            .warnings()
            .first()
            .expect("the reviewer must be told drafts are not saved");
        assert_eq!(warning.summary, "Drafts are not being saved");
        assert!(warning.remediation.is_some());
        // The diff is still reviewable.
        assert_eq!(loaded.session.files().len(), 1);
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
