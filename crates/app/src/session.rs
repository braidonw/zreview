//! The whole of one review sitting, from what is loaded and drafted to what would
//! be posted.

use std::{
    ops::RangeInclusive,
    sync::{Arc, atomic::AtomicBool},
};

use domain::{
    AnchorLocation, DiffAnchor, FindingAcceptance, FindingId, Findings, LoadStage, LoadedSession,
    ReviewEvent, ReviewSession, ReviewStateSink, ReviewSubmission, ReviewSubmitter, SessionFailure,
    SubmissionOutcome,
};

use crate::review::{FindingDisposition, ReviewModel, ReviewRunState};

/// The session state machine PLAN section 9 calls for.
///
/// A window opens on [`SessionPhase::Loading`] before any Git or GitHub work
/// starts, so a slow or failing load is something the reviewer watches rather
/// than a terminal they may not be looking at.
pub enum SessionPhase {
    Loading {
        /// What is being opened, known from the request alone.
        description: String,
        /// The stage the loader last reported.
        stage: String,
    },
    /// Boxed because a whole review dwarfs the other phases, and this enum is
    /// moved between them.
    Ready(Box<ReviewModel>),
    Failed(SessionFailure),
}

/// How far a submission has got.
///
/// `Confirming` exists because nothing may be posted without an explicit human
/// action. It holds the exact request that will be sent, so what the reviewer
/// approves is what leaves the machine, not a re-derivation of it.
pub enum SubmissionState {
    Idle,
    Confirming(Box<ReviewSubmission>),
    Sending,
    Sent(SubmissionOutcome),
    Failed(SessionFailure),
}

/// A confirmed review and where to post it, handed out to be sent.
///
/// Posting blocks on the network, so it is the one thing this model refuses to do
/// itself. The caller sends it and reports back through
/// [`SessionModel::complete_send`].
pub struct PendingSend {
    pub submission: ReviewSubmission,
    pub submitter: Arc<dyn ReviewSubmitter>,
}

/// Everything one review sitting is, and every action that changes it.
///
/// Held behind a lock and shared by whatever is displaying it. Effects come back
/// as return values rather than callbacks. A caller learns what to redraw from
/// what a method hands back, so nothing here has to know a view exists.
pub struct SessionModel {
    phase: SessionPhase,
    /// Where draft changes are written, once the session is ready.
    review_sink: Option<Box<dyn ReviewStateSink>>,
    /// Where a confirmed review is posted. Absent when the session is not a pull
    /// request, in which case submitting is not offered at all.
    submitter: Option<Arc<dyn ReviewSubmitter>>,
    submission: SubmissionState,
}

impl SessionModel {
    /// Starts on the loading phase, for a request that has not begun yet.
    #[must_use]
    pub fn loading(description: impl Into<String>) -> Self {
        Self {
            phase: SessionPhase::Loading {
                description: description.into(),
                stage: LoadStage::default().label().to_owned(),
            },
            review_sink: None,
            submitter: None,
            submission: SubmissionState::Idle,
        }
    }

    /// Records the stage the loader has reached, reporting whether it moved.
    ///
    /// Ignored once the session has finished, so a late report cannot drag a ready
    /// or failed session back to loading.
    #[must_use]
    pub fn set_stage(&mut self, label: impl Into<String>) -> bool {
        let SessionPhase::Loading { stage, .. } = &mut self.phase else {
            return false;
        };
        let label = label.into();
        if *stage == label {
            return false;
        }
        *stage = label;
        true
    }

    /// Moves to the loaded session, or to the failure that stopped it.
    pub fn finish(&mut self, result: Result<LoadedSession, SessionFailure>) {
        self.phase = match result {
            Ok(loaded) => {
                self.review_sink = loaded.review_sink;
                self.submitter = loaded.submitter;
                SessionPhase::Ready(Box::new(ReviewModel::new(loaded.session)))
            }
            Err(failure) => SessionPhase::Failed(failure),
        };
    }

    #[must_use]
    pub const fn phase(&self) -> &SessionPhase {
        &self.phase
    }

    #[must_use]
    pub const fn is_loading(&self) -> bool {
        matches!(self.phase, SessionPhase::Loading { .. })
    }

    /// Why the session could not be loaded, if it could not.
    #[must_use]
    pub const fn failure(&self) -> Option<&SessionFailure> {
        match &self.phase {
            SessionPhase::Failed(failure) => Some(failure),
            SessionPhase::Loading { .. } | SessionPhase::Ready(_) => None,
        }
    }

    /// The loaded review, once there is one.
    #[must_use]
    pub fn review(&self) -> Option<&ReviewModel> {
        match &self.phase {
            SessionPhase::Ready(review) => Some(review),
            SessionPhase::Loading { .. } | SessionPhase::Failed(_) => None,
        }
    }

    /// The reason drafts are not reaching storage, if they are not.
    #[must_use]
    pub fn draft_write_failure(&self) -> Option<String> {
        self.review_sink.as_ref().and_then(|sink| sink.failure())
    }

    /// Stores what a composer holds over a span of rows, and writes it through.
    ///
    /// Storage is asked to save on every keystroke, which is what makes the text
    /// survive a crash; a sink is required not to block, so this stays cheap.
    /// Reports whether the span was actually stored, not merely constructible.
    /// A span whose ends fall in different hunks builds an anchor but is refused
    /// by `set_draft_over`, and that refusal must reach the caller too.
    pub fn draft_edited(&mut self, rows: RangeInclusive<usize>, body: String) -> bool {
        let Self {
            phase, review_sink, ..
        } = self;
        let SessionPhase::Ready(review) = phase else {
            return false;
        };
        let file = review.session.selected_file_index();
        // The anchor is read before the change so an emptied draft still reports
        // which position it was removed from. It covers the whole span, so a range
        // is persisted and cleared as one comment.
        let Some(anchor) = review.session.anchor_for_span(file, rows.clone()) else {
            return false;
        };
        let stored = review.session.set_draft_over(file, rows, body);
        if stored {
            Self::record_draft(review_sink.as_deref(), &review.session, &anchor);
        }
        stored
    }

    /// Removes the draft on a row and tells storage it is gone.
    pub fn draft_discarded(&mut self, row: usize) -> bool {
        let Self {
            phase, review_sink, ..
        } = self;
        let SessionPhase::Ready(review) = phase else {
            return false;
        };
        let file = review.session.selected_file_index();
        let Some(anchor) = review.session.anchor_for_span(file, row..=row) else {
            return false;
        };
        review.session.clear_draft(file, row);
        Self::record_draft(review_sink.as_deref(), &review.session, &anchor);
        true
    }

    /// Moves a stale draft onto a row of the current diff.
    ///
    /// Moving a draft touches two positions, so storage hears both. The old one is
    /// now empty and the new one holds the text. Persistence needs both or the
    /// draft would come back twice.
    pub fn draft_reanchored(&mut self, stale: &DiffAnchor, row: usize) -> bool {
        let Self {
            phase, review_sink, ..
        } = self;
        let SessionPhase::Ready(review) = phase else {
            return false;
        };
        let file = review.session.selected_file_index();
        let Some(moved) = review.session.reanchor_draft(stale, file, row) else {
            return false;
        };
        // The position it left, then the one it now occupies.
        if let Some(sink) = review_sink.as_deref() {
            sink.discard(&moved.vacated);
        }
        Self::record_draft(review_sink.as_deref(), &review.session, &moved.anchored);
        true
    }

    /// Stores the review summary and writes it through.
    pub fn summary_edited(&mut self, body: String) {
        let Self {
            phase, review_sink, ..
        } = self;
        let SessionPhase::Ready(review) = phase else {
            return;
        };
        review.session.set_summary(body);
        Self::record_summary(review_sink.as_deref(), &review.session);
    }

    /// Accepts a finding, saying what is left for the view to do.
    pub fn accept_finding(&mut self, id: FindingId) -> FindingDisposition {
        let Self {
            phase, review_sink, ..
        } = self;
        let SessionPhase::Ready(review) = phase else {
            return FindingDisposition::Unknown;
        };
        let disposition = match review.session.accept_finding(id) {
            FindingAcceptance::Drafted { anchor, body } => {
                let provenance = review
                    .session
                    .drafts()
                    .get(&anchor)
                    .and_then(|draft| draft.provenance.clone());
                review.reselect_finding();
                if let (Some(sink), Some(provenance)) = (review_sink.as_deref(), provenance) {
                    // Text first, then where it came from: an attribution without
                    // the comment it belongs to is worth nothing.
                    sink.save(&anchor, &body);
                    sink.save_provenance(&anchor, &provenance);
                }
                FindingDisposition::Drafted
            }
            FindingAcceptance::Occupied {
                location,
                existing,
                proposed,
                ..
            } => {
                // The reviewer already wrote something here. Both texts go to the
                // composer and they decide; nothing is saved until they do.
                review.selected_finding = Some(id);
                FindingDisposition::Composer {
                    location,
                    seed: format!("{}\n\n{proposed}", existing.trim_end()),
                }
            }
            FindingAcceptance::NotInline { proposed } => {
                // Nowhere to anchor it, so it belongs in the review summary, which the reviewer can still edit or delete before submitting.
                let existing = review.session.summary().trim_end().to_owned();
                let merged = if existing.is_empty() {
                    proposed
                } else {
                    format!("{existing}\n\n{proposed}")
                };
                review.session.set_summary(merged.clone());
                review.session.retire_finding(id);
                review.reselect_finding();
                Self::record_summary(review_sink.as_deref(), &review.session);
                FindingDisposition::Summary { body: merged }
            }
            FindingAcceptance::Unknown => FindingDisposition::Unknown,
        };
        if !matches!(disposition, FindingDisposition::Unknown) {
            review.touch();
        }
        disposition
    }

    /// Forces a finding onto its anchor, overwriting the reviewer's own draft
    /// there.
    ///
    /// Only reached after the reviewer has been asked and chosen to replace what
    /// they wrote; `accept_finding` refuses this on its own. Reports whether a
    /// pending finding still had that id.
    pub fn overwrite_finding(&mut self, id: FindingId) -> bool {
        let Self {
            phase, review_sink, ..
        } = self;
        let SessionPhase::Ready(review) = phase else {
            return false;
        };
        let Some((anchor, body)) = review.session.overwrite_finding(id) else {
            return false;
        };
        let provenance = review
            .session
            .drafts()
            .get(&anchor)
            .and_then(|draft| draft.provenance.clone());
        review.reselect_finding();
        if let (Some(sink), Some(provenance)) = (review_sink.as_deref(), provenance) {
            sink.save(&anchor, &body);
            sink.save_provenance(&anchor, &provenance);
        }
        review.touch();
        true
    }

    /// Rejects a claim and remembers the decision, so a re-run does not offer it
    /// again.
    pub fn dismiss_finding(&mut self, id: FindingId) -> bool {
        let Self {
            phase, review_sink, ..
        } = self;
        let SessionPhase::Ready(review) = phase else {
            return false;
        };
        let Some(fingerprint) = review.session.dismiss_finding(id) else {
            return false;
        };
        review.reselect_finding();
        if let (Some(sink), Some(head_sha)) =
            (review_sink.as_deref(), review.session.source().head_sha())
        {
            sink.dismiss_finding(head_sha, &fingerprint);
        }
        review.touch();
        true
    }

    /// Selects a finding, returning where the diff should scroll to show it.
    ///
    /// `None` for a finding about the change as a whole, which has nowhere to
    /// scroll to.
    pub fn reveal_finding(&mut self, id: FindingId) -> Option<AnchorLocation> {
        let SessionPhase::Ready(review) = &mut self.phase else {
            return None;
        };
        review.selected_finding = Some(id);
        review
            .session
            .findings()
            .get(id)
            .and_then(|finding| finding.location)
    }

    /// The session a review would run against, when one can be started.
    ///
    /// Absent while the session is loading or failed, and while a run is already in
    /// flight.
    #[must_use]
    pub fn review_request(&self) -> Option<ReviewSession> {
        let SessionPhase::Ready(review) = &self.phase else {
            return None;
        };
        (!review.run.is_running()).then(|| review.session.clone())
    }

    /// Records that a review run has started, with the flag that stops it.
    pub fn review_started(&mut self, cancel: Arc<AtomicBool>) {
        let SessionPhase::Ready(review) = &mut self.phase else {
            return;
        };
        review.run = ReviewRunState::Running {
            detail: "Starting...".to_owned(),
            cancel,
        };
        review.touch();
    }

    /// Publishes the backend's latest progress line, reporting whether it changed.
    ///
    /// Ignored unless a run is in flight, so a late report cannot make a finished
    /// review look like it is still going.
    #[must_use]
    pub fn review_progress(&mut self, line: impl Into<String>) -> bool {
        let SessionPhase::Ready(review) = &mut self.phase else {
            return false;
        };
        let ReviewRunState::Running { detail, .. } = &mut review.run else {
            return false;
        };
        let line = line.into();
        if *detail == line {
            return false;
        }
        *detail = line;
        review.touch();
        true
    }

    /// Takes the findings a completed run produced.
    ///
    /// `unreviewed` names files the run did not see (excluded, or too large to fit
    /// in the material). They are kept so a partial review cannot present itself as
    /// a complete one.
    pub fn review_finished(&mut self, findings: Findings, unreviewed: Vec<String>) {
        let SessionPhase::Ready(review) = &mut self.phase else {
            return;
        };
        let rejected = findings.rejected().len();
        let suppressed = review.session.set_findings(findings);
        let accepted = review.session.findings().len();
        review.reselect_finding();
        review.run = ReviewRunState::Complete {
            accepted,
            rejected,
            suppressed,
            unreviewed,
        };
        // The disclosure has served its purpose; the findings are what the reviewer
        // wants the space for now. The summary line stays visible either way.
        review.guidance_expanded = false;
        review.touch();
    }

    /// Reports a run that produced nothing, with what to do about it.
    pub fn review_failed(&mut self, summary: impl Into<String>, remediation: Option<String>) {
        let SessionPhase::Ready(review) = &mut self.phase else {
            return;
        };
        review.run = ReviewRunState::Failed {
            summary: summary.into(),
            remediation,
        };
        review.touch();
    }

    /// Asks the running review to stop.
    pub fn cancel_review(&self) {
        let SessionPhase::Ready(review) = &self.phase else {
            return;
        };
        review.run.cancel();
    }

    /// Opens or closes the guidance section.
    pub fn toggle_guidance_panel(&mut self) {
        let SessionPhase::Ready(review) = &mut self.phase else {
            return;
        };
        review.guidance_expanded = !review.guidance_expanded;
        review.touch();
    }

    /// Turns one guidance file on or off for the next run.
    ///
    /// Not persisted: it is a decision about this sitting, and a choice that
    /// silently outlived the session would be a worse surprise than re-making it.
    pub fn toggle_guidance(&mut self, path: &str) -> bool {
        let SessionPhase::Ready(review) = &mut self.phase else {
            return false;
        };
        let toggled = review.session.toggle_guidance(path).is_some();
        if toggled {
            review.touch();
        }
        toggled
    }

    /// Switches the displayed file, reporting whether it moved.
    pub fn select_file(&mut self, index: usize) -> bool {
        let SessionPhase::Ready(review) = &mut self.phase else {
            return false;
        };
        review.session.select_file(index)
    }

    /// Marks the open file seen, or unmarks it.
    pub fn toggle_viewed(&mut self) {
        let SessionPhase::Ready(review) = &mut self.phase else {
            return;
        };
        review.session.toggle_selected_viewed();
    }

    /// Assembles what submitting would post and holds it for approval.
    ///
    /// Deliberately stops here. PLAN requires that nothing is ever posted without
    /// an explicit human submission action, so building the request and sending it
    /// are two separate steps with a person in between.
    pub fn request_submission(&mut self, event: ReviewEvent) {
        let SessionPhase::Ready(review) = &self.phase else {
            return;
        };
        self.submission = match review.session.prepare_submission(event) {
            Ok(submission) => SubmissionState::Confirming(Box::new(submission)),
            Err(refused) => SubmissionState::Failed(
                SessionFailure::new("This review cannot be submitted yet")
                    .with_remediation(refused.to_string()),
            ),
        };
    }

    pub fn cancel_submission(&mut self) {
        self.submission = SubmissionState::Idle;
    }

    /// Takes the confirmed review, so it can be posted away from the UI thread.
    ///
    /// Only a confirmation with somewhere to post it produces one, and only once. A
    /// second call while the first is in flight hands back nothing, so a double
    /// click cannot post twice.
    #[must_use]
    pub fn begin_send(&mut self) -> Option<PendingSend> {
        let (SubmissionState::Confirming(submission), Some(submitter), SessionPhase::Ready(_)) =
            (&self.submission, self.submitter.clone(), &self.phase)
        else {
            return None;
        };
        let submission = (**submission).clone();
        self.submission = SubmissionState::Sending;
        Some(PendingSend {
            submission,
            submitter,
        })
    }

    /// Records what the forge said, and forgets what it accepted.
    ///
    /// Local drafts are cleared only once the review has landed. Until then the
    /// local copy is the only copy.
    ///
    /// # Panics
    ///
    /// Panics if the phase is not `Ready`. `begin_send` only ever hands out a
    /// submission while it is, and the phase cannot regress.
    pub fn complete_send(
        &mut self,
        submission: &ReviewSubmission,
        posted: Result<SubmissionOutcome, SessionFailure>,
    ) {
        let outcome = match posted {
            Ok(outcome) => outcome,
            // Nothing local was touched, so every draft is still there.
            Err(failure) => {
                self.submission = SubmissionState::Failed(failure);
                return;
            }
        };
        let Self {
            phase, review_sink, ..
        } = self;
        let SessionPhase::Ready(review) = phase else {
            panic!("a submission completed while the session was not ready");
        };
        // Only now is it safe to forget them.
        let anchors = submission.submitted_anchors();
        review.session.mark_submitted(submission);
        if let (Some(sink), Some(head_sha)) =
            (review_sink.as_deref(), review.session.source().head_sha())
        {
            sink.clear_submitted(head_sha, &anchors);
        }
        self.submission = SubmissionState::Sent(outcome);
    }

    /// How far a submission has got.
    #[must_use]
    pub const fn submission(&self) -> &SubmissionState {
        &self.submission
    }

    /// Tells storage what now sits at an anchor, which may be nothing.
    fn record_draft(
        sink: Option<&dyn ReviewStateSink>,
        session: &ReviewSession,
        anchor: &DiffAnchor,
    ) {
        let Some(sink) = sink else {
            return;
        };
        match session.drafts().get(anchor) {
            Some(draft) => sink.save(anchor, &draft.body),
            None => sink.discard(anchor),
        }
    }

    /// Tells storage the summary, when there is a snapshot to key it by.
    ///
    /// The generated fixture has no head commit, so it has nowhere to record one.
    fn record_summary(sink: Option<&dyn ReviewStateSink>, session: &ReviewSession) {
        if let (Some(sink), Some(head_sha)) = (sink, session.source().head_sha()) {
            sink.save_summary(head_sha, session.summary());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, atomic::Ordering};

    use domain::{
        ChangeCounts, DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffSide, FileStatus,
        FindingProvenance, GuidanceSelection, SessionSource, SubmissionOutcome,
    };

    use super::*;
    use crate::review::ReviewRunState;

    /// Records what a session asks storage to do.
    #[derive(Clone, Default)]
    struct RecordingSink {
        calls: Arc<Mutex<Vec<String>>>,
        failure: Option<String>,
    }

    impl RecordingSink {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ReviewStateSink for RecordingSink {
        fn save(&self, anchor: &DiffAnchor, body: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("save {} {} {body}", anchor.path, anchor.line));
        }

        fn discard(&self, anchor: &DiffAnchor) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("discard {} {}", anchor.path, anchor.line));
        }

        fn save_summary(&self, _head_sha: &str, body: &str) {
            self.calls.lock().unwrap().push(format!("summary {body}"));
        }

        fn save_provenance(&self, anchor: &DiffAnchor, provenance: &FindingProvenance) {
            self.calls.lock().unwrap().push(format!(
                "provenance {} {} {}",
                anchor.path, anchor.line, provenance.origin
            ));
        }

        fn dismiss_finding(&self, _head_sha: &str, fingerprint: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("dismiss {fingerprint}"));
        }

        fn clear_submitted(&self, _head_sha: &str, anchors: &[DiffAnchor]) {
            let positions = anchors
                .iter()
                .map(|anchor| format!("{} {}", anchor.path, anchor.line))
                .collect::<Vec<_>>()
                .join(", ");
            self.calls
                .lock()
                .unwrap()
                .push(format!("clear submitted [{positions}]"));
        }

        fn failure(&self) -> Option<String> {
            self.failure.clone()
        }
    }

    /// Records every submission attempt, so a test can prove one did not happen.
    #[derive(Clone, Default)]
    struct RecordingSubmitter {
        posted: Arc<Mutex<Vec<ReviewSubmission>>>,
        failure: Option<SessionFailure>,
    }

    impl RecordingSubmitter {
        fn posted(&self) -> Vec<ReviewSubmission> {
            self.posted.lock().unwrap().clone()
        }
    }

    impl ReviewSubmitter for RecordingSubmitter {
        fn submit(
            &self,
            submission: &ReviewSubmission,
        ) -> Result<SubmissionOutcome, SessionFailure> {
            self.posted.lock().unwrap().push(submission.clone());
            self.failure.clone().map_or_else(
                || {
                    Ok(SubmissionOutcome {
                        state: "COMMENTED".to_owned(),
                        url: "https://github.com/acme/widgets/pull/42".to_owned(),
                        comment_count: submission.comments.len(),
                    })
                },
                Err,
            )
        }
    }

    fn repository_backed_session(paths: &[&str]) -> ReviewSession {
        let head_sha: Arc<str> = "a".repeat(40).into();
        let files = paths
            .iter()
            .map(|path| {
                let mut file = DiffFile::demo(40);
                file.path = (*path).into();
                file
            })
            .collect::<Vec<_>>();
        ReviewSession::new(
            SessionSource::LocalComparison {
                repository_root: std::path::PathBuf::from("/tmp/repository"),
                base_sha: Arc::clone(&head_sha),
                diff_base_sha: Arc::clone(&head_sha),
                head_sha,
            },
            files.into(),
        )
        .unwrap()
    }

    /// A session with one file split across two hunks, so a span crossing them
    /// can be tested.
    fn two_hunk_session() -> ReviewSession {
        let head_sha: Arc<str> = "a".repeat(40).into();
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(10),
                new_line: Some(10),
                text: "a".into(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(11),
                text: "b".into(),
            },
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(80),
                new_line: Some(80),
                text: "c".into(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(81),
                text: "d".into(),
            },
        ];
        let file = DiffFile {
            path: "src/review.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![
                DiffHunk {
                    header: "@@ -10,1 +10,2 @@".into(),
                    old_start: 10,
                    new_start: 10,
                    line_range: 0..2,
                },
                DiffHunk {
                    header: "@@ -80,1 +80,2 @@".into(),
                    old_start: 80,
                    new_start: 80,
                    line_range: 2..4,
                },
            ]
            .into(),
            counts: ChangeCounts::of(&lines),
            lines: lines.into(),
        };
        ReviewSession::new(
            SessionSource::LocalComparison {
                repository_root: std::path::PathBuf::from("/tmp/repository"),
                base_sha: Arc::clone(&head_sha),
                diff_base_sha: Arc::clone(&head_sha),
                head_sha,
            },
            vec![file].into(),
        )
        .unwrap()
    }

    fn submittable_session() -> ReviewSession {
        let head_sha: Arc<str> = "a".repeat(40).into();
        let mut file = DiffFile::demo(40);
        file.path = "src/review.rs".into();
        ReviewSession::new(
            SessionSource::GitHubPullRequest {
                repository_root: std::path::PathBuf::from("/tmp/repository"),
                owner: "acme".into(),
                repository: "widgets".into(),
                number: 42,
                title: "Improve the review flow".into(),
                url: "https://github.com/acme/widgets/pull/42".into(),
                base_ref: "main".into(),
                head_ref: "feature".into(),
                base_sha: Arc::clone(&head_sha),
                recorded_base_sha: Arc::clone(&head_sha),
                diff_base_sha: Arc::clone(&head_sha),
                head_sha,
            },
            vec![file].into(),
        )
        .unwrap()
    }

    fn loaded_model(
        session: ReviewSession,
        review_sink: Option<Box<dyn ReviewStateSink>>,
        submitter: Option<Arc<dyn ReviewSubmitter>>,
    ) -> SessionModel {
        let mut model = SessionModel::loading("a review");
        model.finish(Ok(LoadedSession {
            session,
            review_sink,
            submitter,
        }));
        model
    }

    /// A model on a repository-backed snapshot of one file.
    fn ready_model(review_sink: Option<Box<dyn ReviewStateSink>>) -> SessionModel {
        loaded_model(
            repository_backed_session(&["src/review.rs"]),
            review_sink,
            None,
        )
    }

    /// The loaded review, which every one of these tests has.
    fn review(model: &SessionModel) -> &ReviewModel {
        model.review().expect("the session should be ready")
    }

    /// Puts one finding on the review, validated against the snapshot.
    fn give_finding(model: &mut SessionModel, raw: domain::RawFinding) -> FindingId {
        let anchors = review(model).session().anchors().expect("anchored").clone();
        let findings = Findings::validate(
            vec![raw],
            &anchors,
            &domain::FindingOrigin::Ai("claude-code".into()),
        );
        let id = findings.accepted()[0].id;
        model.review_finished(findings, Vec::new());
        id
    }

    /// Puts one finding on the review, anchored to the row a comment can go on.
    fn give_one_finding(model: &mut SessionModel, title: &str) -> FindingId {
        let anchor = review(model)
            .session()
            .anchor_for(0, 1)
            .expect("row 1 can carry a comment");
        give_finding(
            model,
            domain::RawFinding {
                location: Some(domain::RawLocation {
                    path: anchor.path.clone(),
                    side: anchor.side,
                    line: anchor.line,
                    start_line: None,
                }),
                severity: domain::Severity::Warning,
                confidence: 0.8,
                title: title.to_owned(),
                rationale: "because".to_owned(),
                proposed_comment: "Handle the failure here.".to_owned(),
                guidance_sources: vec![domain::GuidanceCitation {
                    path: "AGENTS.md".into(),
                    content_hash: "hash".into(),
                }],
            },
        )
    }

    fn guidance_selection() -> GuidanceSelection {
        let entry = |path: &str, content: &str| domain::GuidanceEntry {
            excerpt: domain::GuidanceExcerpt {
                path: path.into(),
                scope: "whole repository".into(),
                content: content.to_owned(),
                content_hash: "hash".into(),
            },
            included: true,
        };
        GuidanceSelection::new(
            vec![
                entry("AGENTS.md", &"a".repeat(2048)),
                entry("CLAUDE.md", "b"),
            ],
            vec![domain::GuidanceSkip {
                path: "HUGE.md".into(),
                reason: "90000 bytes, over the 65536-byte limit".into(),
            }],
            vec!["vendor/lib.rs".into()],
        )
    }

    /// A window opens before loading starts, so the model has to begin in a state
    /// that has no session at all.
    #[test]
    fn a_session_starts_loading_then_becomes_ready() {
        let mut model = SessionModel::loading("pull request #42");

        assert!(model.is_loading());
        assert!(model.failure().is_none());
        assert!(model.review().is_none());

        assert!(model.set_stage(LoadStage::BuildingDiff.label()));
        assert!(
            !model.set_stage(LoadStage::BuildingDiff.label()),
            "unchanged"
        );
        assert!(model.is_loading());

        model.finish(Ok(LoadedSession::unsaved(repository_backed_session(&[
            "src/review.rs",
        ]))));

        assert!(!model.is_loading());
        assert!(model.failure().is_none());
        assert!(model.review().is_some());
    }

    #[test]
    fn accepting_a_finding_writes_the_draft_and_its_provenance_to_the_sink() {
        let sink = RecordingSink::default();
        let mut model = ready_model(Some(Box::new(sink.clone())));
        let id = give_one_finding(&mut model, "unchecked index");

        assert_eq!(model.accept_finding(id), FindingDisposition::Drafted);

        // The text is written before the note about where it came from.
        assert_eq!(
            sink.calls(),
            [
                "save src/review.rs 2 Handle the failure here.".to_owned(),
                "provenance src/review.rs 2 claude-code".to_owned(),
            ]
        );
        let session = review(&model).session();
        assert!(session.findings().is_empty(), "acted on");
        let draft = session.draft_at(0, 1).expect("the draft is there");
        assert!(draft.is_proposed());
    }

    #[test]
    fn dismissing_a_finding_records_the_decision() {
        let sink = RecordingSink::default();
        let mut model = ready_model(Some(Box::new(sink.clone())));
        let id = give_one_finding(&mut model, "unchecked index");

        assert!(model.dismiss_finding(id));

        assert_eq!(sink.calls().len(), 1);
        assert!(sink.calls()[0].starts_with("dismiss "));
        assert!(review(&model).session().findings().is_empty());
        assert!(review(&model).session().drafts().is_empty());
    }

    /// The reviewer's own words are never overwritten; both texts go to the
    /// composer instead, and nothing is saved until they commit.
    #[test]
    fn accepting_onto_an_occupied_line_opens_the_composer_pre_filled() {
        let sink = RecordingSink::default();
        let mut model = ready_model(Some(Box::new(sink.clone())));

        // The reviewer writes on the line first.
        assert!(model.draft_edited(1..=1, "mine".to_owned()));
        let id = give_one_finding(&mut model, "unchecked index");
        let before = sink.calls().len();

        let FindingDisposition::Composer { location, seed } = model.accept_finding(id) else {
            panic!("an occupied line has to be handed back to the reviewer");
        };

        assert_eq!(location, AnchorLocation { file: 0, row: 1 });
        assert_eq!(seed, "mine\n\nHandle the failure here.");
        let session = review(&model).session();
        // Untouched, and the finding is still waiting.
        assert_eq!(
            session.draft_at(0, 1).map(|draft| draft.body.as_str()),
            Some("mine")
        );
        assert_eq!(session.findings().len(), 1);
        assert_eq!(sink.calls().len(), before, "nothing was written");
    }

    /// The desktop panel's alternative to the composer: overwrite rather than
    /// merge, chosen only once the reviewer has been asked.
    #[test]
    fn overwriting_a_findings_own_line_replaces_the_reviewers_draft() {
        let sink = RecordingSink::default();
        let mut model = ready_model(Some(Box::new(sink.clone())));
        assert!(model.draft_edited(1..=1, "mine".to_owned()));
        let id = give_one_finding(&mut model, "unchecked index");
        let before = sink.calls().len();

        assert!(model.overwrite_finding(id));

        assert_eq!(
            &sink.calls()[before..],
            [
                "save src/review.rs 2 Handle the failure here.".to_owned(),
                "provenance src/review.rs 2 claude-code".to_owned(),
            ]
        );
        let session = review(&model).session();
        assert!(session.findings().is_empty(), "acted on");
        let draft = session.draft_at(0, 1).expect("the draft is there");
        assert_eq!(draft.body, "Handle the failure here.");
        assert!(draft.is_proposed());
    }

    #[test]
    fn overwriting_an_unknown_finding_changes_nothing() {
        let mut model = ready_model(None);

        assert!(!model.overwrite_finding(FindingId(7)));
    }

    #[test]
    fn a_finding_about_the_whole_change_goes_into_the_summary() {
        let sink = RecordingSink::default();
        let mut model = ready_model(Some(Box::new(sink.clone())));
        let id = give_finding(
            &mut model,
            domain::RawFinding {
                location: None,
                severity: domain::Severity::Info,
                confidence: 0.5,
                title: "no tests".to_owned(),
                rationale: String::new(),
                proposed_comment: "Consider adding a test.".to_owned(),
                guidance_sources: Vec::new(),
            },
        );

        assert_eq!(
            model.accept_finding(id),
            FindingDisposition::Summary {
                body: "Consider adding a test.".to_owned()
            }
        );

        let session = review(&model).session();
        assert_eq!(session.summary(), "Consider adding a test.");
        assert!(session.drafts().is_empty(), "nowhere to anchor it");
        assert!(session.findings().is_empty());
        assert!(sink.calls().iter().any(|call| call.starts_with("summary ")));
    }

    /// The guidance panel is the disclosure notice, so turning a file off has to
    /// change what a run would send, not just how the row is drawn.
    #[test]
    fn toggling_guidance_changes_what_would_be_sent() {
        let mut session = repository_backed_session(&["src/review.rs"]);
        session.set_guidance(guidance_selection());
        let mut model = loaded_model(session, None, None);

        let guidance = review(&model).session().guidance();
        assert_eq!(guidance.included_count(), 2);
        assert_eq!(guidance.included_bytes(), 2049);

        assert!(model.toggle_guidance("AGENTS.md"));

        let guidance = review(&model).session().guidance();
        assert_eq!(guidance.included_count(), 1);
        assert_eq!(guidance.included_bytes(), 1);
        let sent: Vec<_> = guidance
            .included()
            .map(|excerpt| excerpt.path.to_string())
            .collect();
        assert_eq!(sent, vec!["CLAUDE.md".to_owned()]);
    }

    #[test]
    fn the_guidance_section_starts_open_and_collapses_once_a_run_finishes() {
        let mut model = ready_model(None);

        // Open before a run: PLAN wants what will be sent seen before it is sent.
        assert!(review(&model).guidance_expanded());

        model.review_finished(Findings::default(), Vec::new());
        assert!(!review(&model).guidance_expanded());

        // And it can be reopened.
        model.toggle_guidance_panel();
        assert!(review(&model).guidance_expanded());
    }

    /// The panel carries the only Review button, so it must not depend on there
    /// being something to show yet.
    #[test]
    fn the_panel_is_reachable_whenever_a_review_is_possible() {
        // A repository-backed snapshot can be reviewed, so the panel (and the only
        // Review button) must be reachable before anything is discovered.
        assert!(review(&ready_model(None)).findings_panel_visible());

        let mut session = repository_backed_session(&["src/review.rs"]);
        session.set_guidance(guidance_selection());
        assert!(review(&loaded_model(session, None, None)).findings_panel_visible());
    }

    /// The fixture has no commit, so there is nothing to review and no panel.
    #[test]
    fn the_generated_fixture_offers_no_review() {
        let session = ReviewSession::new(SessionSource::Demo, vec![DiffFile::demo(8)].into())
            .expect("the fixture has files");

        assert!(!review(&loaded_model(session, None, None)).findings_panel_visible());
    }

    #[test]
    fn cancelling_sets_the_flag_the_backend_polls() {
        let mut model = ready_model(None);
        let cancel = Arc::new(AtomicBool::new(false));

        model.review_started(Arc::clone(&cancel));
        assert!(review(&model).run().is_running());
        assert!(
            model.review_request().is_none(),
            "a run is already in flight"
        );

        model.cancel_review();
        assert!(cancel.load(Ordering::Relaxed));
    }

    /// Asserts the revision moved, and hands back the new one.
    fn bumped(model: &SessionModel, previous: u32, what: &str) -> u32 {
        let revision = review(model).revision();
        assert!(revision > previous, "{what} did not bump the revision");
        revision
    }

    /// The panel reaches a front end from several commands at once, and one that
    /// read the model before a change can be delivered after it. Dropping the
    /// stale one needs a number that only ever goes up.
    #[test]
    fn every_change_the_panel_shows_bumps_the_revision() {
        let mut session = repository_backed_session(&["src/review.rs"]);
        session.set_guidance(guidance_selection());
        let mut model = loaded_model(session, None, None);
        let mut revision = review(&model).revision();

        model.toggle_guidance_panel();
        revision = bumped(&model, revision, "opening the guidance section");

        assert!(model.toggle_guidance("AGENTS.md"));
        revision = bumped(&model, revision, "turning a guidance file off");

        model.review_started(Arc::new(AtomicBool::new(false)));
        revision = bumped(&model, revision, "starting a run");

        assert!(model.review_progress("Reading the diff"));
        revision = bumped(&model, revision, "a progress line");

        assert!(!model.review_progress("Reading the diff"));
        assert_eq!(
            review(&model).revision(),
            revision,
            "a line that did not move is not a change"
        );

        model.review_failed("claude is not installed", None);
        revision = bumped(&model, revision, "a failed run");

        model.review_started(Arc::new(AtomicBool::new(false)));
        revision = bumped(&model, revision, "starting a second run");

        model.review_finished(Findings::default(), Vec::new());
        revision = bumped(&model, revision, "a completed run");

        let id = give_one_finding(&mut model, "risky() is unchecked");
        revision = bumped(&model, revision, "findings arriving");

        assert!(model.dismiss_finding(id));
        revision = bumped(&model, revision, "dismissing a finding");

        let id = give_one_finding(&mut model, "still risky");
        revision = bumped(&model, revision, "findings arriving again");

        assert_eq!(model.accept_finding(id), FindingDisposition::Drafted);
        revision = bumped(&model, revision, "accepting a finding");

        let id = give_one_finding(&mut model, "occupying its own line");
        assert!(model.overwrite_finding(id));
        bumped(&model, revision, "overwriting a finding");
    }

    /// The panel shows the latest line, so a report that arrives once the run is
    /// over must not make a finished review look like it is still going.
    #[test]
    fn progress_lands_only_while_a_run_is_in_flight() {
        let mut model = ready_model(None);

        assert!(
            !model.review_progress("Starting claude"),
            "nothing is running yet"
        );

        model.review_started(Arc::new(AtomicBool::new(false)));
        assert!(model.review_progress("Starting claude"));
        assert!(
            !model.review_progress("Starting claude"),
            "the same line again is not worth a redraw"
        );
        let ReviewRunState::Running { detail, .. } = review(&model).run() else {
            panic!("the run should be in flight");
        };
        assert_eq!(detail, "Starting claude");

        model.review_finished(Findings::default(), Vec::new());
        assert!(!model.review_progress("Reading the diff"));
    }

    /// A run that skipped files must not present itself as having covered the
    /// change, and a run that suppressed claims must not look like one that
    /// found nothing.
    #[test]
    fn a_completed_run_keeps_its_counts_and_what_it_did_not_see() {
        let mut model = ready_model(None);
        model.review_started(Arc::new(AtomicBool::new(false)));

        model.review_finished(
            Findings::default(),
            vec!["vendor/lib.rs".to_owned(), "huge.json".to_owned()],
        );

        let ReviewRunState::Complete {
            accepted,
            rejected,
            suppressed,
            unreviewed,
        } = review(&model).run()
        else {
            panic!("the run should have completed");
        };
        assert_eq!(*accepted, 0);
        assert_eq!(*rejected, 0);
        assert_eq!(*suppressed, 0);
        assert_eq!(unreviewed, &["vendor/lib.rs", "huge.json"]);
    }

    #[test]
    fn a_failed_run_keeps_its_remediation() {
        let mut model = ready_model(None);

        model.review_failed("claude is not installed", Some("Install it.".to_owned()));

        let ReviewRunState::Failed {
            summary,
            remediation,
        } = review(&model).run()
        else {
            panic!("the run should have failed");
        };
        assert_eq!(summary, "claude is not installed");
        assert_eq!(remediation.as_deref(), Some("Install it."));
    }

    /// The last link in the chain: an edit reaching storage.
    #[test]
    fn draft_changes_reach_the_sink() {
        let sink = RecordingSink::default();
        let mut model = ready_model(Some(Box::new(sink.clone())));

        assert!(model.draft_edited(0..=0, "h".to_owned()));
        assert!(model.draft_edited(0..=0, "hi".to_owned()));

        // One write per keystroke, each with the text so far.
        assert_eq!(
            sink.calls(),
            [
                "save src/review.rs 1 h".to_owned(),
                "save src/review.rs 1 hi".to_owned(),
            ],
        );
    }

    #[test]
    fn discarding_a_draft_reaches_the_sink() {
        let sink = RecordingSink::default();
        let mut model = ready_model(Some(Box::new(sink.clone())));

        assert!(model.draft_edited(0..=0, "x".to_owned()));
        assert!(model.draft_edited(0..=0, String::new()));

        // An emptied draft is a removal, not a blank comment.
        assert_eq!(
            sink.calls().last().map(String::as_str),
            Some("discard src/review.rs 1"),
        );

        // And so is one the reviewer threw away outright.
        assert!(model.draft_edited(0..=0, "second thoughts".to_owned()));
        assert!(model.draft_discarded(0));
        assert_eq!(
            sink.calls().last().map(String::as_str),
            Some("discard src/review.rs 1"),
        );
        assert!(review(&model).session().drafts().is_empty());
    }

    /// Writes failing means work is being lost as it is typed, so the model has to
    /// be able to say so.
    #[test]
    fn a_failing_sink_is_reported() {
        let sink = RecordingSink {
            failure: Some("disk is full".to_owned()),
            ..RecordingSink::default()
        };
        let model = ready_model(Some(Box::new(sink)));

        assert_eq!(model.draft_write_failure(), Some("disk is full".to_owned()));
    }

    #[test]
    fn a_session_without_a_sink_still_accepts_drafts() {
        let mut model = ready_model(None);

        assert!(model.draft_edited(0..=0, "in memory only".to_owned()));

        assert_eq!(model.draft_write_failure(), None);
        assert_eq!(
            review(&model)
                .session()
                .draft_at(0, 0)
                .map(|draft| draft.body.clone()),
            Some("in memory only".to_owned()),
        );
    }

    #[test]
    fn a_failed_load_shows_its_remediation_instead_of_a_session() {
        let mut model = SessionModel::loading("pull request #42");
        let failure = SessionFailure::from_error(
            "GitHub is not authenticated",
            &std::io::Error::other("The token in GH_TOKEN is invalid."),
        )
        .with_remediation("Run `gh auth login`, then reopen the pull request.");

        model.finish(Err(failure));

        assert!(!model.is_loading());
        assert!(model.review().is_none());
        let shown = model.failure().expect("the failure should be shown");
        assert_eq!(shown.summary, "GitHub is not authenticated");
        assert!(
            shown
                .remediation
                .as_ref()
                .unwrap()
                .contains("gh auth login")
        );
        assert!(shown.detail.as_ref().unwrap().contains("GH_TOKEN"));
    }

    /// A stage report can arrive after the load finished; it must not drag a ready
    /// or failed session back to loading.
    #[test]
    fn a_late_stage_report_cannot_reopen_a_finished_session() {
        let mut model = SessionModel::loading("pull request #42");

        model.finish(Err(SessionFailure::new("Could not load")));
        assert!(!model.set_stage(LoadStage::BuildingDiff.label()));

        assert!(!model.is_loading());
        assert!(model.failure().is_some());
    }

    #[test]
    fn a_conversation_load_failure_is_visible_on_a_ready_session() {
        let mut session = repository_backed_session(&["src/review.rs"]);
        session.push_warning(SessionFailure::new("GitHub's rate limit is exhausted"));
        let model = loaded_model(session, None, None);

        // Carried on the session, so a sidebar cannot silently show zero
        // conversations as though there were none.
        assert_eq!(
            review(&model)
                .session()
                .warnings()
                .first()
                .map(|warning| warning.summary.clone()),
            Some("GitHub's rate limit is exhausted".to_owned()),
        );
    }

    /// A comment over a span is one range draft, not several single-line ones.
    #[test]
    fn a_span_of_rows_becomes_one_range_comment() {
        let mut model = ready_model(None);

        // Rows 0..=2 are context lines in the fixture's single hunk.
        assert!(model.draft_edited(0..=2, "this block".to_owned()));

        let session = review(&model).session();
        let draft = session
            .draft_at(0, 2)
            .expect("the draft is keyed at the span's last row");
        assert_eq!(draft.body, "this block");
        assert!(draft.anchor.is_multiline());
        assert_eq!(draft.anchor.start_line, Some(1));
        assert_eq!(draft.anchor.line, 3);
        assert_eq!(session.drafts().len(), 1);
    }

    /// A span whose ends fall in different hunks builds an anchor but is refused
    /// by `set_draft_over`; that refusal must reach the caller, not `true`.
    #[test]
    fn a_span_crossing_hunks_is_refused_and_stores_nothing() {
        let sink = RecordingSink::default();
        let mut model = loaded_model(two_hunk_session(), Some(Box::new(sink.clone())), None);

        // Row 0 is in the first hunk, row 3 in the second.
        assert!(!model.draft_edited(0..=3, "spans two hunks".to_owned()));

        assert!(review(&model).session().drafts().is_empty());
        assert!(sink.calls().is_empty(), "nothing should have been written");
    }

    /// The point of the whole path: what is written becomes a draft anchored to the
    /// row, without the reviewer doing anything to save it.
    #[test]
    fn an_edited_draft_is_stored_anchored_to_its_row() {
        let mut model = ready_model(None);

        // Row 6 of the fixture is an addition, so it anchors on the right.
        assert!(model.draft_edited(6..=6, "needs a test".to_owned()));

        let session = review(&model).session();
        let draft = session
            .draft_at(0, 6)
            .expect("the edit should have stored a draft");
        assert_eq!(draft.body, "needs a test");
        assert_eq!(draft.anchor.side, DiffSide::Right);
        assert_eq!(draft.anchor.path.as_ref(), "src/review.rs");
        assert!(!draft.is_stale);
        assert_eq!(session.drafts().len(), 1);
    }

    #[test]
    fn emptying_a_draft_removes_it() {
        let mut model = ready_model(None);

        assert!(model.draft_edited(0..=0, "oops".to_owned()));
        assert_eq!(review(&model).session().drafts().len(), 1);

        assert!(model.draft_edited(0..=0, String::new()));

        assert!(review(&model).session().drafts().is_empty());
    }

    /// The property the whole design turns on: asking to submit posts nothing.
    #[test]
    fn requesting_submission_does_not_post_anything() {
        let submitter = RecordingSubmitter::default();
        let mut session = submittable_session();
        session.set_draft(0, 6, "needs a test");
        session.set_summary("Two notes.");
        let mut model = loaded_model(session, None, Some(Arc::new(submitter.clone())));

        model.request_submission(ReviewEvent::Comment);

        // Waiting on a human, not on the network.
        assert!(
            submitter.posted().is_empty(),
            "nothing may be posted before confirmation",
        );
        let SubmissionState::Confirming(submission) = model.submission() else {
            panic!("the review should be waiting for confirmation");
        };
        assert_eq!(submission.event, ReviewEvent::Comment);
        assert_eq!(submission.comments.len(), 1);
        assert_eq!(submission.body, "Two notes.");
    }

    #[test]
    fn confirming_posts_the_review_and_clears_what_was_sent() {
        let submitter = RecordingSubmitter::default();
        let sink = RecordingSink::default();
        let mut session = submittable_session();
        session.set_draft(0, 6, "needs a test");
        session.set_summary("Two notes.");
        let mut model = loaded_model(
            session,
            Some(Box::new(sink.clone())),
            Some(Arc::new(submitter.clone())),
        );

        model.request_submission(ReviewEvent::Comment);
        let pending = model.begin_send().expect("the review is confirmed");
        assert!(
            model.begin_send().is_none(),
            "a second confirmation must not post twice",
        );
        // The hop a caller makes off the UI thread, made on this one.
        let posted = pending.submitter.submit(&pending.submission);
        model.complete_send(&pending.submission, posted);

        let posted = submitter.posted();
        assert_eq!(posted.len(), 1, "posted exactly once");
        assert_eq!(posted[0].comments.len(), 1);
        assert_eq!(posted[0].head_sha.as_ref(), "a".repeat(40));

        assert!(
            matches!(model.submission(), SubmissionState::Sent(_)),
            "should report success",
        );
        let session = review(&model).session();
        // Forgotten only after the forge accepted it.
        assert!(session.drafts().is_empty());
        assert_eq!(session.summary(), "");
        assert!(
            sink.calls()
                .iter()
                .any(|call| call.starts_with("clear submitted [src/review.rs 6")),
            "storage should be told what was posted: {:?}",
            sink.calls(),
        );
    }

    /// A failed submission must leave every draft exactly where it was.
    #[test]
    fn a_failed_submission_keeps_every_draft() {
        let submitter = RecordingSubmitter {
            failure: Some(
                SessionFailure::new("The pull request moved on")
                    .with_remediation("Your drafts are unchanged."),
            ),
            ..RecordingSubmitter::default()
        };
        let sink = RecordingSink::default();
        let mut session = submittable_session();
        session.set_draft(0, 6, "needs a test");
        session.set_summary("Two notes.");
        let mut model = loaded_model(
            session,
            Some(Box::new(sink.clone())),
            Some(Arc::new(submitter.clone())),
        );

        model.request_submission(ReviewEvent::Comment);
        let pending = model.begin_send().expect("the review is confirmed");
        let posted = pending.submitter.submit(&pending.submission);
        model.complete_send(&pending.submission, posted);

        let SubmissionState::Failed(failure) = model.submission() else {
            panic!("the failure should be shown");
        };
        assert_eq!(failure.summary, "The pull request moved on");
        let session = review(&model).session();
        assert_eq!(session.drafts().len(), 1, "the draft is still here");
        assert_eq!(session.summary(), "Two notes.", "so is the summary");
        assert!(
            !sink.calls().iter().any(|call| call.contains("clear")),
            "nothing may be cleared when the post failed: {:?}",
            sink.calls(),
        );
    }

    #[test]
    fn a_review_that_cannot_be_assembled_explains_itself_without_posting() {
        let submitter = RecordingSubmitter::default();
        // A comment review with a draft but no summary: GitHub requires a body.
        let mut session = submittable_session();
        session.set_draft(0, 6, "needs a test");
        let mut model = loaded_model(session, None, Some(Arc::new(submitter.clone())));

        model.request_submission(ReviewEvent::Comment);

        let SubmissionState::Failed(failure) = model.submission() else {
            panic!("the review should be refused");
        };
        assert!(
            failure
                .remediation
                .as_ref()
                .unwrap()
                .contains("needs a summary"),
            "unexpected remediation: {:?}",
            failure.remediation,
        );
        assert!(model.begin_send().is_none());
        assert!(submitter.posted().is_empty());
    }

    #[test]
    fn cancelling_a_confirmation_posts_nothing() {
        let submitter = RecordingSubmitter::default();
        let mut session = submittable_session();
        session.set_summary("Just a note.");
        let mut model = loaded_model(session, None, Some(Arc::new(submitter.clone())));

        model.request_submission(ReviewEvent::Approve);
        model.cancel_submission();

        assert!(submitter.posted().is_empty());
        assert!(matches!(model.submission(), SubmissionState::Idle));
        assert!(model.begin_send().is_none());
        // The summary survives cancelling.
        assert_eq!(review(&model).session().summary(), "Just a note.");
    }

    /// Editing the summary must reach both the session and storage.
    #[test]
    fn the_summary_is_stored_as_it_is_typed() {
        let sink = RecordingSink::default();
        let mut model = loaded_model(submittable_session(), Some(Box::new(sink.clone())), None);

        model.summary_edited("o".to_owned());
        model.summary_edited("ok".to_owned());

        assert_eq!(review(&model).session().summary(), "ok");
        assert_eq!(
            sink.calls(),
            ["summary o".to_owned(), "summary ok".to_owned()],
        );
    }

    /// Moving a stale draft has to reach storage as a removal *and* a write, or
    /// reopening would show the draft in both places.
    #[test]
    fn re_anchoring_a_stale_draft_reaches_the_sink_from_both_sides() {
        let sink = RecordingSink::default();
        let mut session = repository_backed_session(&["src/review.rs"]);
        let stale = DiffAnchor {
            path: "src/review.rs".into(),
            side: DiffSide::Right,
            line: 9_999,
            start_line: None,
            head_sha: "a".repeat(40).into(),
        };
        session.restore_drafts([(stale.clone(), "written last week".to_owned())]);
        assert_eq!(session.drafts().stale_count(), 1);
        let mut model = loaded_model(session, Some(Box::new(sink.clone())), None);

        // Row 6 can carry a comment, so the stale text can move onto it.
        assert!(model.draft_reanchored(&stale, 6));

        let session = review(&model).session();
        assert_eq!(session.drafts().stale_count(), 0, "no longer stale");
        assert_eq!(
            session.draft_at(0, 6).map(|draft| draft.body.clone()),
            Some("written last week".to_owned()),
        );
        assert_eq!(session.drafts().len(), 1, "moved, not duplicated");

        assert_eq!(
            sink.calls(),
            [
                // The position it left, then the one it now occupies.
                "discard src/review.rs 9999".to_owned(),
                "save src/review.rs 6 written last week".to_owned(),
            ],
        );
    }
}
