//! One loaded review, and how far the run that populates it has got.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use domain::{AnchorLocation, FindingId, ReviewSession};

/// How far a review run has got.
#[derive(Clone, Debug, Default)]
pub enum ReviewRunState {
    /// No review has been asked for, or the last one has been dealt with.
    #[default]
    Idle,
    Running {
        /// The backend's most recent progress line.
        detail: String,
        /// Set to stop the run. Shared with the background thread, which polls it
        /// between steps, so cancelling costs nothing until the backend next looks.
        cancel: Arc<AtomicBool>,
    },
    /// The run finished. Counts are kept so the panel can report the shape of the
    /// outcome even when nothing was accepted.
    Complete {
        accepted: usize,
        rejected: usize,
        /// Claims suppressed because the reviewer dismissed them before.
        suppressed: usize,
        /// Files the review did not see.
        unreviewed: Vec<String>,
    },
    Failed {
        summary: String,
        remediation: Option<String>,
    },
}

impl ReviewRunState {
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    /// Asks a running review to stop.
    pub fn cancel(&self) {
        if let Self::Running { cancel, .. } = self {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// What accepting a finding leaves for a view to do.
///
/// Accepting is the one place the model cannot finish the job alone. A finding
/// that would overwrite the reviewer's own words has to be put in front of them
/// instead, and only a view can do that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindingDisposition {
    /// It became a draft. The diff needs the new drafts.
    Drafted,
    /// The line already holds the reviewer's words, so nothing was written. Both
    /// texts go to a composer at `location`, and the reviewer decides.
    Composer {
        location: AnchorLocation,
        seed: String,
    },
    /// It was about the change as a whole, so it went into the summary, which now
    /// reads this.
    Summary { body: String },
    /// No pending finding has that id, or the session is not ready.
    Unknown,
}

/// The review a reviewer is working through.
///
/// Owned by [`SessionModel`] once a session is loaded, which is also what mutates
/// it. The fields here are the state, and every change to them is paired with
/// whatever storage has to hear about it.
///
/// [`SessionModel`]: crate::SessionModel
pub struct ReviewModel {
    pub(crate) session: ReviewSession,
    /// How far the current review run has got.
    pub(crate) run: ReviewRunState,
    /// Which finding the reviewer is looking at.
    pub(crate) selected_finding: Option<FindingId>,
    /// Whether the guidance section is open.
    ///
    /// Open before the first run, because PLAN wants what will be sent seen before
    /// it is sent; collapsed afterwards, when the findings are what matters.
    pub(crate) guidance_expanded: bool,
}

impl ReviewModel {
    pub(crate) fn new(session: ReviewSession) -> Self {
        Self {
            session,
            run: ReviewRunState::Idle,
            selected_finding: None,
            guidance_expanded: true,
        }
    }

    #[must_use]
    pub const fn session(&self) -> &ReviewSession {
        &self.session
    }

    #[must_use]
    pub const fn run(&self) -> &ReviewRunState {
        &self.run
    }

    #[must_use]
    pub const fn selected_finding(&self) -> Option<FindingId> {
        self.selected_finding
    }

    #[must_use]
    pub const fn guidance_expanded(&self) -> bool {
        self.guidance_expanded
    }

    /// Whether the findings panel has anything worth the screen space.
    ///
    /// Shown whenever a review is possible at all, which means whenever the
    /// snapshot has anchors to validate findings against. An earlier version
    /// required guidance or findings to exist first, which hid the panel. It also hid
    /// the only Review button, on any repository that happens to carry no
    /// `AGENTS.md`. The feature was invisible exactly where a reviewer had no other
    /// way to discover it.
    ///
    /// The generated fixture has no commit, so it cannot be reviewed and gets
    /// nothing.
    #[must_use]
    pub fn findings_panel_visible(&self) -> bool {
        self.session.anchors().is_some()
            || !matches!(self.run, ReviewRunState::Idle)
            || !self.session.findings().is_empty()
    }

    /// The finding after the selected one, wrapping round to the first.
    ///
    /// `None` when there is nothing to move to.
    #[must_use]
    pub fn next_finding(&self) -> Option<FindingId> {
        let findings = self.session.findings();
        if findings.is_empty() {
            return None;
        }
        findings
            .accepted()
            .iter()
            .position(|finding| Some(finding.id) == self.selected_finding)
            .and_then(|position| findings.accepted().get(position + 1))
            .or_else(|| findings.accepted().first())
            .map(|finding| finding.id)
    }

    /// Moves the selection off a finding that has been acted on.
    pub(crate) fn reselect_finding(&mut self) {
        self.selected_finding = self
            .session
            .findings()
            .accepted()
            .first()
            .map(|finding| finding.id);
    }
}
