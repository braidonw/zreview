//! Running an automated review over a loaded session.
//!
//! The pieces already exist separately: guidance discovery reads what a repository
//! wants, a backend produces claims, and the anchor validator decides which of them
//! could be a comment. This is the order they go in, and the only place that knows
//! all three — so the UI can ask for a review without knowing that any of them are
//! involved.
//!
//! Like [`load`], this runs subprocesses and must not be called on a UI thread.
//!
//! [`load`]: crate::load
//!
//! The stages follow PLAN section 8, minus one deliberately: deterministic checks
//! are not run. Those execute repository-provided commands, which PLAN says needs a
//! one-time trust decision first, and there is nowhere to make that decision yet.
//! Discovering a repository's configuration is not consent to run its commands, so
//! until that gate exists this reviews with a model and nothing else.

use domain::{
    FindingOrigin, Findings, ReviewBackend, ReviewError, ReviewEventSink, ReviewRequest,
    ReviewSession, SessionSource,
};

/// One completed review, and everything needed to explain it.
#[derive(Clone, Debug)]
pub struct ReviewRun {
    /// Findings that survived validation, ranked, and the claims that did not.
    pub findings: Findings,
    /// Files excluded by `.zreview.toml`, so an unreviewed file is not mistaken
    /// for a clean one.
    pub excluded: Vec<String>,
    /// Files the backend was not shown because the material would not fit.
    pub omitted: Vec<String>,
}

impl ReviewRun {
    /// Whether the review saw the whole change.
    #[must_use]
    pub fn was_complete(&self) -> bool {
        self.excluded.is_empty() && self.omitted.is_empty()
    }
}

/// Discovers guidance, runs the backend, and validates what it returns.
///
/// # Errors
///
/// Returns a [`ReviewError`] when the backend could not produce output. A run that
/// completes with no findings is a success — it means nothing was found, which is a
/// legitimate answer and a common one.
pub fn run(
    session: &ReviewSession,
    backend: &dyn ReviewBackend,
    events: &dyn ReviewEventSink,
) -> Result<ReviewRun, ReviewError> {
    let Some(anchors) = session.anchors() else {
        // Only a repository-backed session has anchors, and without them nothing a
        // backend said could become a comment.
        return Err(ReviewError::NothingToReview);
    };
    let (head_sha, base_sha) = session
        .source()
        .head_sha()
        .zip(session.source().diff_base_sha())
        .ok_or(ReviewError::NothingToReview)?;

    // Read from the session rather than re-discovered. The guidance panel showed
    // the reviewer what would be sent and let them turn things off, so a run that
    // discovered its own copy could quietly disagree with the disclosure they were
    // given.
    let guidance = session.guidance();

    let mut excluded = Vec::new();
    let mut files = Vec::new();
    for file in session.files() {
        if guidance.excludes(&file.path) {
            excluded.push(file.path.to_string());
        } else {
            files.push(file.clone());
        }
    }
    if files.is_empty() {
        return Err(ReviewError::NothingToReview);
    }

    let request = ReviewRequest {
        head_sha: head_sha.clone(),
        base_sha: base_sha.clone(),
        title: pull_request_title(session.source()),
        description: None,
        guidance: guidance.included().cloned().collect(),
        files: files.into(),
    };

    let material = review::material::render(&request);
    let raw = backend.review(&request, events)?;
    let origin = FindingOrigin::Ai(backend.name());

    Ok(ReviewRun {
        findings: Findings::validate(raw, anchors, &origin),
        excluded,
        omitted: material.omitted,
    })
}

/// The change's stated intent, when the source records one.
///
/// A local comparison has no description of itself, so a review of one is judged
/// against the guidance alone.
fn pull_request_title(source: &SessionSource) -> Option<String> {
    match source {
        SessionSource::GitHubPullRequest { title, .. } => Some(title.to_string()),
        SessionSource::Demo | SessionSource::LocalComparison { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::{Arc, Mutex},
    };

    use domain::{
        ChangeCounts, DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStatus, IgnoreProgress,
        RawFinding, RawLocation, ReviewProgress, Severity,
    };

    use super::*;

    /// A backend that returns whatever it was built with and records the request.
    struct Stub {
        findings: Vec<RawFinding>,
        seen: Mutex<Option<ReviewRequest>>,
    }

    impl Stub {
        fn new(findings: Vec<RawFinding>) -> Self {
            Self {
                findings,
                seen: Mutex::new(None),
            }
        }

        fn request(&self) -> ReviewRequest {
            self.seen
                .lock()
                .expect("the lock is not poisoned")
                .clone()
                .expect("the backend was called")
        }
    }

    impl ReviewBackend for Stub {
        fn name(&self) -> Arc<str> {
            Arc::from("stub")
        }

        fn review(
            &self,
            request: &ReviewRequest,
            _events: &dyn ReviewEventSink,
        ) -> Result<Vec<RawFinding>, ReviewError> {
            *self.seen.lock().expect("the lock is not poisoned") = Some(request.clone());
            Ok(self.findings.clone())
        }
    }

    const HEAD: &str = "head000000000000";
    const BASE: &str = "base000000000000";

    fn file(path: &str) -> DiffFile {
        let lines = vec![DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(1),
            text: Arc::from("let x = risky();"),
        }];
        DiffFile {
            path: Arc::from(path),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            hunks: Arc::from(vec![DiffHunk {
                header: Arc::from("@@ -0,0 +1 @@"),
                old_start: 0,
                new_start: 1,
                line_range: 0..1,
            }]),
            counts: ChangeCounts::of(&lines),
            lines: Arc::from(lines),
        }
    }

    /// A session with guidance attached, the way `load` builds one.
    ///
    /// Discovery happens at load time now, so a session that skipped it would make
    /// `run` behave as though the repository said nothing.
    fn session(root: &Path, paths: &[&str]) -> ReviewSession {
        let files: Vec<_> = paths.iter().map(|path| file(path)).collect();
        let mut session = ReviewSession::new(
            SessionSource::LocalComparison {
                repository_root: root.to_path_buf(),
                base_sha: Arc::from(BASE),
                diff_base_sha: Arc::from(BASE),
                head_sha: Arc::from(HEAD),
            },
            Arc::from(files),
        )
        .expect("the session has files");
        let discovered = review::discover(root, paths);
        session.set_guidance(review::into_selection(&discovered, paths));
        session
    }

    fn finding(path: &str, line: u32) -> RawFinding {
        RawFinding {
            location: Some(RawLocation {
                path: Arc::from(path),
                side: domain::DiffSide::Right,
                line,
                start_line: None,
            }),
            severity: Severity::Warning,
            confidence: 0.8,
            title: "risky() is unchecked".to_owned(),
            rationale: "it can fail".to_owned(),
            proposed_comment: "Handle the failure.".to_owned(),
            guidance_sources: Vec::new(),
        }
    }

    #[test]
    fn a_valid_finding_comes_back_anchored_to_the_session() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let session = session(root.path(), &["src/main.rs"]);
        let backend = Stub::new(vec![finding("src/main.rs", 1)]);

        let run = run(&session, &backend, &IgnoreProgress).expect("the review completes");

        assert_eq!(run.findings.accepted().len(), 1);
        let accepted = &run.findings.accepted()[0];
        assert_eq!(&*accepted.snapshot, HEAD);
        assert!(accepted.is_inline());
        assert_eq!(accepted.origin, FindingOrigin::Ai(Arc::from("stub")));
        assert!(run.was_complete());
    }

    #[test]
    fn a_finding_on_a_line_that_is_not_in_the_diff_is_rejected_not_shown() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let session = session(root.path(), &["src/main.rs"]);
        let backend = Stub::new(vec![finding("src/main.rs", 999)]);

        let run = run(&session, &backend, &IgnoreProgress).expect("the review completes");

        assert!(run.findings.accepted().is_empty());
        assert_eq!(run.findings.rejected().len(), 1);
    }

    #[test]
    fn discovered_guidance_reaches_the_backend_with_its_hash() {
        let root = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(root.path().join("AGENTS.md"), "Never use unwrap.")
            .expect("the guidance is written");
        let session = session(root.path(), &["src/main.rs"]);
        let backend = Stub::new(Vec::new());

        let run = run(&session, &backend, &IgnoreProgress).expect("the review completes");

        let sent = backend.request();
        let excerpt = sent
            .guidance
            .iter()
            .find(|excerpt| &*excerpt.path == "AGENTS.md")
            .expect("the guidance was sent");
        assert!(excerpt.content.contains("Never use unwrap"));
        assert!(!excerpt.content_hash.is_empty());
        assert_eq!(run.findings.accepted().len(), 0);
    }

    /// The guidance panel is the input to a run, not a preview of one. Turning a
    /// file off has to actually stop it being sent, or the panel is a disclosure
    /// notice that can disagree with what left the machine.
    #[test]
    fn guidance_the_reviewer_turned_off_is_not_sent() {
        let root = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(root.path().join("AGENTS.md"), "Never use unwrap.")
            .expect("the guidance is written");
        std::fs::write(root.path().join("CLAUDE.md"), "Prefer iterators.")
            .expect("the guidance is written");
        let mut session = session(root.path(), &["src/main.rs"]);
        let backend = Stub::new(Vec::new());

        assert!(session.set_guidance_included("CLAUDE.md", false));
        run(&session, &backend, &IgnoreProgress).expect("the review completes");

        let sent: Vec<_> = backend
            .request()
            .guidance
            .iter()
            .map(|excerpt| excerpt.path.to_string())
            .collect();
        assert_eq!(sent, vec!["AGENTS.md".to_owned()]);
    }

    #[test]
    fn turning_all_guidance_off_still_reviews_against_the_diff() {
        let root = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(root.path().join("AGENTS.md"), "Never use unwrap.")
            .expect("the guidance is written");
        let mut session = session(root.path(), &["src/main.rs"]);
        let backend = Stub::new(vec![finding("src/main.rs", 1)]);

        session.set_guidance_included("AGENTS.md", false);
        let run = run(&session, &backend, &IgnoreProgress).expect("the review completes");

        assert!(backend.request().guidance.is_empty());
        // A review with no guidance is still a review; bugs are bugs.
        assert_eq!(run.findings.accepted().len(), 1);
    }

    #[test]
    fn excluded_files_are_not_reviewed_and_are_reported_as_unreviewed() {
        let root = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            root.path().join(".zreview.toml"),
            "[review]\nexclude_files = [\"vendor/**\"]\n",
        )
        .expect("the config is written");
        let session = session(root.path(), &["src/main.rs", "vendor/lib.rs"]);
        let backend = Stub::new(Vec::new());

        let run = run(&session, &backend, &IgnoreProgress).expect("the review completes");

        assert_eq!(run.excluded, vec!["vendor/lib.rs".to_owned()]);
        assert_eq!(backend.request().paths(), vec!["src/main.rs"]);
        // A review that skipped a file did not review the whole change.
        assert!(!run.was_complete());
    }

    #[test]
    fn a_snapshot_with_every_file_excluded_does_not_run_the_backend() {
        let root = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            root.path().join(".zreview.toml"),
            "[review]\nexclude_files = [\"**\"]\n",
        )
        .expect("the config is written");
        let session = session(root.path(), &["src/main.rs"]);
        let backend = Stub::new(vec![finding("src/main.rs", 1)]);

        let error = run(&session, &backend, &IgnoreProgress).expect_err("nothing is reviewable");

        assert!(matches!(error, ReviewError::NothingToReview));
        assert!(
            backend
                .seen
                .lock()
                .expect("the lock is not poisoned")
                .is_none()
        );
    }

    #[test]
    fn the_demo_session_has_nothing_to_review_against() {
        let session = ReviewSession::new(SessionSource::Demo, Arc::from(vec![DiffFile::demo(4)]))
            .expect("demo");
        let backend = Stub::new(Vec::new());

        let error =
            run(&session, &backend, &IgnoreProgress).expect_err("the fixture has no commit");

        assert!(matches!(error, ReviewError::NothingToReview));
    }

    #[test]
    fn a_backend_failure_is_passed_through_rather_than_becoming_an_empty_review() {
        struct Failing;
        impl ReviewBackend for Failing {
            fn name(&self) -> Arc<str> {
                Arc::from("failing")
            }
            fn review(
                &self,
                _request: &ReviewRequest,
                _events: &dyn ReviewEventSink,
            ) -> Result<Vec<RawFinding>, ReviewError> {
                Err(ReviewError::Unauthenticated {
                    program: Arc::from("claude"),
                })
            }
        }

        let root = tempfile::tempdir().expect("a temporary directory");
        let session = session(root.path(), &["src/main.rs"]);

        let error = run(&session, &Failing, &IgnoreProgress).expect_err("the backend fails");

        assert!(matches!(error, ReviewError::Unauthenticated { .. }));
    }

    #[test]
    fn progress_is_reported_to_whoever_asked() {
        #[derive(Default)]
        struct Recording(Mutex<Vec<String>>);
        impl ReviewEventSink for Recording {
            fn progress(&self, progress: ReviewProgress) {
                self.0
                    .lock()
                    .expect("the lock is not poisoned")
                    .push(progress.to_string());
            }
            fn is_cancelled(&self) -> bool {
                false
            }
        }

        struct Chatty;
        impl ReviewBackend for Chatty {
            fn name(&self) -> Arc<str> {
                Arc::from("chatty")
            }
            fn review(
                &self,
                _request: &ReviewRequest,
                events: &dyn ReviewEventSink,
            ) -> Result<Vec<RawFinding>, ReviewError> {
                events.progress(ReviewProgress::Starting {
                    program: Arc::from("chatty"),
                });
                Ok(Vec::new())
            }
        }

        let root = tempfile::tempdir().expect("a temporary directory");
        let session = session(root.path(), &["src/main.rs"]);
        let events = Recording::default();

        run(&session, &Chatty, &events).expect("the review completes");

        assert_eq!(
            *events.0.lock().expect("the lock is not poisoned"),
            vec!["Starting chatty".to_owned()]
        );
    }
}
