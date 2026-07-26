//! The port a review engine plugs into.
//!
//! Declared here, beside the findings it produces, so that adapters depend on the
//! domain rather than the domain on them — the same direction as [`ReviewStateSink`] and
//! [`ReviewSubmitter`]. A view can show a review running, and report why one
//! failed, without knowing whether a subprocess, an HTTP client, or a stub is
//! behind it.
//!
//! [`ReviewStateSink`]: crate::ReviewStateSink
//! [`ReviewSubmitter`]: crate::ReviewSubmitter
//!
//! What this port deliberately does not carry, per PLAN section 8:
//!
//! - **No credential.** A [`ReviewRequest`] holds review material and nothing
//!   else. The review engine must never be able to post, so it is never given the
//!   means to.
//! - **No write access to the checkout.** A backend is handed the diff it is to
//!   review, not a repository to go and change.
//!
//! The trait is synchronous because the whole application is: a review runs on a
//! background thread and reports progress through [`ReviewEventSink`], the same
//! shape session loading already uses. Reviews are slow — minutes, not
//! milliseconds — so calling one on the UI thread would freeze the window, which
//! is why nothing in the UI crate is given a backend directly.

use std::{
    fmt::{Display, Formatter},
    sync::Arc,
};

use crate::{DiffFile, GuidanceCitation};

/// One guidance file as a backend receives it.
///
/// The content travels with the hash so that a finding citing this guidance can be
/// checked later against what was actually sent, rather than against whatever the
/// file says by then.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuidanceExcerpt {
    /// Repository-relative path, which is also how a finding cites it.
    pub path: Arc<str>,
    /// What it applies to, already rendered for display.
    pub scope: Arc<str>,
    pub content: String,
    pub content_hash: Arc<str>,
}

impl GuidanceExcerpt {
    /// The citation a finding would use to point at this guidance.
    #[must_use]
    pub fn citation(&self) -> GuidanceCitation {
        GuidanceCitation {
            path: Arc::clone(&self.path),
            content_hash: Arc::clone(&self.content_hash),
        }
    }
}

/// Everything a backend is given for one review.
///
/// Bounded by construction: the caller has already applied guidance size limits and
/// file exclusions, so a backend renders what it is handed rather than deciding how
/// much of the repository to read.
#[derive(Clone, Debug)]
pub struct ReviewRequest {
    /// The commit being reviewed. Findings are pinned to it.
    pub head_sha: Arc<str>,
    /// The merge base the diff was taken against.
    pub base_sha: Arc<str>,
    /// The change's own description of its intent, when there is one. A pull
    /// request title and body are the author's claim about the change, which is
    /// worth reviewing the diff against.
    pub title: Option<String>,
    pub description: Option<String>,
    pub guidance: Vec<GuidanceExcerpt>,
    /// The files to review, already filtered.
    pub files: Arc<[DiffFile]>,
}

impl ReviewRequest {
    /// Paths under review, in order.
    #[must_use]
    pub fn paths(&self) -> Vec<&str> {
        self.files.iter().map(|file| &*file.path).collect()
    }

    /// Citations for every piece of guidance sent, so a backend's output can be
    /// checked against what it was actually given.
    #[must_use]
    pub fn citations(&self) -> Vec<GuidanceCitation> {
        self.guidance
            .iter()
            .map(GuidanceExcerpt::citation)
            .collect()
    }
}

/// What a review is doing, for a progress display.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReviewProgress {
    /// Locating and launching the backend.
    Starting { program: Arc<str> },
    /// Waiting on the backend, with what it is working through.
    Running { detail: Arc<str> },
    /// The backend returned; its output is being checked.
    Validating { returned: usize },
}

impl Display for ReviewProgress {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting { program } => write!(formatter, "Starting {program}"),
            Self::Running { detail } => write!(formatter, "{detail}"),
            Self::Validating { returned } => {
                write!(formatter, "Checking {returned} findings against the diff")
            }
        }
    }
}

/// How a running review talks to whoever asked for it.
///
/// Also how it learns to stop: a reviewer who has moved on should not be waiting
/// on a model, so a backend polls [`is_cancelled`] between steps rather than
/// running to completion regardless.
///
/// [`is_cancelled`]: ReviewEventSink::is_cancelled
pub trait ReviewEventSink: Send + Sync {
    fn progress(&self, progress: ReviewProgress);

    /// Whether the reviewer has abandoned this run.
    fn is_cancelled(&self) -> bool;
}

/// A sink that records nothing and never cancels, for callers that only want the
/// result.
#[derive(Clone, Copy, Debug, Default)]
pub struct IgnoreProgress;

impl ReviewEventSink for IgnoreProgress {
    fn progress(&self, _progress: ReviewProgress) {}

    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Why a review did not produce findings.
///
/// Categorised rather than stringly typed for the same reason the forge's errors
/// are: what a reviewer should do about "the CLI is not installed" and "your
/// balance is empty" are completely different, and a single opaque message makes
/// them look the same.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReviewError {
    /// The backend's program is not on `PATH`.
    NotInstalled { program: Arc<str> },
    /// The program exists but could not be started.
    Launch { program: Arc<str>, message: String },
    /// The backend is installed but not signed in.
    Unauthenticated { program: Arc<str> },
    /// The account has no credit or quota left.
    QuotaExhausted { program: Arc<str>, message: String },
    /// The provider asked for the request to be retried later.
    RateLimited { program: Arc<str>, message: String },
    /// The backend ran too long and was stopped.
    TimedOut { program: Arc<str>, seconds: u64 },
    /// The reviewer cancelled the run.
    Cancelled,
    /// The backend produced output that is not the shape it promised.
    ///
    /// Kept distinct from a backend failure because it means the backend
    /// succeeded and the contract between us and it is what broke.
    MalformedOutput { program: Arc<str>, message: String },
    /// The backend reported a failure of its own.
    Backend { program: Arc<str>, message: String },
    /// There was nothing to review.
    NothingToReview,
}

impl ReviewError {
    #[must_use]
    pub fn program(&self) -> Option<&Arc<str>> {
        match self {
            Self::NotInstalled { program }
            | Self::Launch { program, .. }
            | Self::Unauthenticated { program }
            | Self::QuotaExhausted { program, .. }
            | Self::RateLimited { program, .. }
            | Self::TimedOut { program, .. }
            | Self::MalformedOutput { program, .. }
            | Self::Backend { program, .. } => Some(program),
            Self::Cancelled | Self::NothingToReview => None,
        }
    }

    /// What the reviewer can do about it.
    #[must_use]
    pub fn remediation(&self) -> Option<String> {
        match self {
            Self::NotInstalled { program } => Some(format!(
                "Install {program} and make sure it is on your PATH, or choose another review backend."
            )),
            Self::Unauthenticated { program } => {
                Some(format!("Sign in to {program}, then run the review again."))
            }
            Self::QuotaExhausted { program, .. } => Some(format!(
                "Top up or check the plan on the account {program} is signed in to."
            )),
            Self::RateLimited { .. } => Some("Wait a moment and run the review again.".to_owned()),
            Self::TimedOut { .. } => Some(
                "Review fewer files at once, or raise the timeout in your configuration."
                    .to_owned(),
            ),
            Self::MalformedOutput { program, .. } => Some(format!(
                "This is a bug in how zreview talks to {program}, not something you did wrong."
            )),
            // A launch or backend failure carries the program's own message, which
            // says more than anything generic could; cancellation was deliberate.
            Self::Launch { .. } | Self::Backend { .. } | Self::Cancelled => None,
            Self::NothingToReview => {
                Some("Nothing in this snapshot is reviewable after exclusions.".to_owned())
            }
        }
    }

    /// Whether running the same review again might succeed.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::TimedOut { .. } | Self::Launch { .. }
        )
    }
}

impl Display for ReviewError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled { program } => write!(formatter, "{program} is not installed"),
            Self::Launch { program, message } => {
                write!(formatter, "{program} could not be started: {message}")
            }
            Self::Unauthenticated { program } => write!(formatter, "{program} is not signed in"),
            Self::QuotaExhausted { program, message } => {
                write!(formatter, "{program} has no quota left: {message}")
            }
            Self::RateLimited { program, message } => {
                write!(formatter, "{program} is rate limited: {message}")
            }
            Self::TimedOut { program, seconds } => {
                write!(formatter, "{program} did not finish within {seconds}s")
            }
            Self::Cancelled => formatter.write_str("the review was cancelled"),
            Self::MalformedOutput { program, message } => {
                write!(formatter, "{program} returned unusable output: {message}")
            }
            Self::Backend { program, message } => write!(formatter, "{program} failed: {message}"),
            Self::NothingToReview => formatter.write_str("there is nothing to review"),
        }
    }
}

impl std::error::Error for ReviewError {}

/// A review engine.
///
/// Returns claims, not comments: everything it produces goes through
/// [`Findings::validate`] before a reviewer sees it, and through a reviewer before
/// anything is posted.
///
/// [`Findings::validate`]: crate::Findings::validate
pub trait ReviewBackend: Send + Sync {
    /// How this backend identifies itself in findings and errors.
    fn name(&self) -> Arc<str>;

    /// Reviews the request, reporting progress as it goes.
    ///
    /// # Errors
    ///
    /// Returns a [`ReviewError`] describing what stopped the review. An empty
    /// finding list is a success: it means the backend found nothing.
    fn review(
        &self,
        request: &ReviewRequest,
        events: &dyn ReviewEventSink,
    ) -> Result<Vec<crate::RawFinding>, ReviewError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_backend_says_how_to_get_it() {
        let error = ReviewError::NotInstalled {
            program: Arc::from("claude"),
        };

        assert!(
            error
                .remediation()
                .is_some_and(|text| text.contains("PATH"))
        );
        assert!(!error.is_retryable());
    }

    #[test]
    fn transient_failures_are_retryable_and_permanent_ones_are_not() {
        let rate_limited = ReviewError::RateLimited {
            program: Arc::from("claude"),
            message: "slow down".to_owned(),
        };
        let unauthenticated = ReviewError::Unauthenticated {
            program: Arc::from("claude"),
        };

        assert!(rate_limited.is_retryable());
        assert!(!unauthenticated.is_retryable());
    }

    #[test]
    fn cancellation_offers_no_remediation_and_names_no_program() {
        let error = ReviewError::Cancelled;

        assert_eq!(error.remediation(), None);
        assert_eq!(error.program(), None);
    }

    #[test]
    fn guidance_travels_with_the_hash_a_finding_will_cite() {
        let excerpt = GuidanceExcerpt {
            path: Arc::from("AGENTS.md"),
            scope: Arc::from("whole repository"),
            content: "be terse".to_owned(),
            content_hash: Arc::from("abc123"),
        };

        assert_eq!(
            excerpt.citation(),
            GuidanceCitation {
                path: Arc::from("AGENTS.md"),
                content_hash: Arc::from("abc123"),
            }
        );
    }
}
