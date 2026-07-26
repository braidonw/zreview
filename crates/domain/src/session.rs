//! Vocabulary shared between whatever loads a session and whatever displays it.
//!
//! Loading a review touches Git and a forge, so it is slow and it fails in ways
//! the reviewer can usually act on. These types let the loader say where it is
//! and what went wrong without knowing anything about the UI, and let the UI show
//! it without knowing anything about subprocesses.

use std::fmt::Display;

use std::sync::Arc;

use crate::{ReviewSession, ReviewStateSink, ReviewSubmitter};

/// A session that is ready to review, together with where its drafts are written.
///
/// The sink travels with the session because it is bound to the same scope: the
/// two are only correct together, and pairing them here means no caller can wire
/// a session to the wrong storage.
pub struct LoadedSession {
    pub session: ReviewSession,
    /// Absent when there is nothing to persist to — the generated fixture, or a
    /// database that could not be opened.
    pub review_sink: Option<Box<dyn ReviewStateSink>>,
    /// Absent when the session is not a pull request and so cannot be submitted.
    pub submitter: Option<Arc<dyn ReviewSubmitter>>,
}

impl std::fmt::Debug for LoadedSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoadedSession")
            .field("session", &self.session)
            .field("persists_drafts", &self.review_sink.is_some())
            .field("can_submit", &self.submitter.is_some())
            .finish()
    }
}

impl LoadedSession {
    /// A session with nowhere to persist drafts.
    #[must_use]
    pub fn unsaved(session: ReviewSession) -> Self {
        Self {
            session,
            review_sink: None,
            submitter: None,
        }
    }
}

/// How far session loading has progressed.
///
/// Stages exist so a large pull request does not sit behind an unexplained wait.
/// They are advisory: a loader may skip any that do not apply.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LoadStage {
    #[default]
    Starting,
    CheckingAuthentication,
    ReadingPullRequest,
    FetchingObjects,
    BuildingDiff,
    LoadingConversations,
}

impl LoadStage {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::CheckingAuthentication => "Checking GitHub authentication",
            Self::ReadingPullRequest => "Reading pull request metadata",
            Self::FetchingObjects => "Fetching Git objects",
            Self::BuildingDiff => "Building the diff",
            Self::LoadingConversations => "Loading existing conversations",
        }
    }
}

/// A session that could not be loaded, in the form the reviewer should see it.
///
/// `remediation` is the reason this type exists: PLAN requires actionable
/// guidance rather than a generic failure, and only the layer that produced the
/// error knows what would fix it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionFailure {
    /// One line naming what failed.
    pub summary: String,
    /// A concrete next action, when there is one.
    pub remediation: Option<String>,
    /// Underlying detail, shown but subordinate to the summary.
    pub detail: Option<String>,
}

impl SessionFailure {
    #[must_use]
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            remediation: None,
            detail: None,
        }
    }

    /// Builds a failure from an error, keeping its text as the detail.
    #[must_use]
    pub fn from_error(summary: impl Into<String>, error: &impl Display) -> Self {
        Self {
            summary: summary.into(),
            remediation: None,
            detail: Some(error.to_string()),
        }
    }

    #[must_use]
    pub fn with_remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }

    /// Attaches remediation only when there is some.
    #[must_use]
    pub fn with_optional_remediation(mut self, remediation: Option<impl Into<String>>) -> Self {
        self.remediation = remediation.map(Into::into);
        self
    }
}

impl Display for SessionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.summary)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        if let Some(remediation) = &self.remediation {
            write!(formatter, " ({remediation})")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_has_a_label() {
        for stage in [
            LoadStage::Starting,
            LoadStage::CheckingAuthentication,
            LoadStage::ReadingPullRequest,
            LoadStage::FetchingObjects,
            LoadStage::BuildingDiff,
            LoadStage::LoadingConversations,
        ] {
            assert!(!stage.label().is_empty());
        }
        assert_eq!(LoadStage::default(), LoadStage::Starting);
    }

    #[test]
    fn a_failure_carries_summary_detail_and_remediation() {
        let error = std::io::Error::other("gh exited with status 1");
        let failure = SessionFailure::from_error("Could not read the pull request", &error)
            .with_remediation("Run `gh auth login`.");

        assert_eq!(failure.summary, "Could not read the pull request");
        assert_eq!(failure.detail.as_deref(), Some("gh exited with status 1"));
        assert_eq!(failure.remediation.as_deref(), Some("Run `gh auth login`."));
        assert_eq!(
            failure.to_string(),
            "Could not read the pull request: gh exited with status 1 (Run `gh auth login`.)",
        );
    }

    #[test]
    fn optional_remediation_stays_absent_when_there_is_none() {
        let failure =
            SessionFailure::new("Something failed").with_optional_remediation(None::<String>);
        assert_eq!(failure.remediation, None);
        assert_eq!(failure.to_string(), "Something failed");
    }
}
