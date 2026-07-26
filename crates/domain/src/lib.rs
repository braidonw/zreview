use std::{
    collections::BTreeSet,
    error::Error,
    fmt::{Display, Formatter},
    ops::{Range, RangeInclusive},
    path::PathBuf,
    sync::Arc,
};

mod anchor;
mod backend;
mod comment;
mod draft;
mod finding;
mod session;
mod submission;

pub use anchor::{AnchorError, AnchorIndex, AnchorLocation, DiffAnchor, DiffSide};
pub use backend::{
    GuidanceExcerpt, IgnoreProgress, ReviewBackend, ReviewError, ReviewEventSink, ReviewProgress,
    ReviewRequest,
};
pub use comment::{CommentThread, PlacedComments, ReviewComment, UnplacedReason, UnplacedThread};
pub use draft::{DraftComment, DraftSink, Drafts};
pub use finding::{
    DismissedFindings, Finding, FindingId, FindingOrigin, FindingProvenance, Findings,
    GuidanceCitation, MAX_COMMENT_BYTES, MAX_FINDINGS, MAX_RATIONALE_BYTES, MAX_TITLE_BYTES,
    RawFinding, RawLocation, RejectedFinding, RejectionReason, Severity, fingerprint,
};
pub use session::{LoadStage, LoadedSession, SessionFailure};
pub use submission::{
    ExcludedDraft, ExclusionReason, ReviewEvent, ReviewSubmission, ReviewSubmitter,
    SubmissionOutcome, SubmissionRefused, SubmittableComment,
};

/// The semantic role of one row in a unified diff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
    NoNewlineMarker,
}

impl DiffLineKind {
    #[must_use]
    pub const fn marker(self) -> char {
        match self {
            Self::Context => ' ',
            Self::Addition => '+',
            Self::Deletion => '-',
            Self::NoNewlineMarker => '\\',
        }
    }
}

/// One source line with stable coordinates on both sides of a diff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: Arc<str>,
}

/// Metadata locating a hunk in a flattened [`DiffFile::lines`] collection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffHunk {
    pub header: Arc<str>,
    pub old_start: u32,
    pub new_start: u32,
    pub line_range: Range<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
}

/// How many lines a file adds and removes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChangeCounts {
    pub additions: usize,
    pub deletions: usize,
}

impl ChangeCounts {
    /// Counts the additions and deletions in a set of diff lines.
    #[must_use]
    pub fn of(lines: &[DiffLine]) -> Self {
        let mut counts = Self::default();
        for line in lines {
            match line.kind {
                DiffLineKind::Addition => counts.additions += 1,
                DiffLineKind::Deletion => counts.deletions += 1,
                DiffLineKind::Context | DiffLineKind::NoNewlineMarker => {}
            }
        }
        counts
    }
}

/// A reviewable file. Lines are flat to make virtualized indexing constant-time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffFile {
    pub path: Arc<str>,
    pub old_path: Option<Arc<str>>,
    pub status: FileStatus,
    pub is_binary: bool,
    pub hunks: Arc<[DiffHunk]>,
    pub lines: Arc<[DiffLine]>,
    /// Counted once when the file is built.
    ///
    /// The file sidebar draws these on every frame, and recomputing them meant
    /// walking every line of every visible file — a 100,000-line file cost a
    /// 100,000-element scan per frame while it was on screen.
    pub counts: ChangeCounts,
}

impl DiffFile {
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    #[must_use]
    pub fn line(&self, index: usize) -> Option<&DiffLine> {
        self.lines.get(index)
    }

    /// The header of the hunk that begins at this row, if one does.
    ///
    /// Headers ride on the row that starts their hunk rather than occupying rows
    /// of their own, so row indices stay equal to line indices. Anchors, drafts,
    /// and comment threads are all keyed by row, and inserting header rows would
    /// shift every one of them.
    #[must_use]
    pub fn hunk_header_at(&self, row: usize) -> Option<&Arc<str>> {
        self.hunks
            .binary_search_by_key(&row, |hunk| hunk.line_range.start)
            .ok()
            .map(|index| &self.hunks[index].header)
    }

    /// The hunk a row belongs to.
    #[must_use]
    pub fn hunk_at(&self, row: usize) -> Option<&DiffHunk> {
        let candidate = self
            .hunks
            .partition_point(|hunk| hunk.line_range.start <= row)
            .checked_sub(1)?;
        self.hunks
            .get(candidate)
            .filter(|hunk| hunk.line_range.contains(&row))
    }

    /// Why this file shows no diff rows, when it shows none.
    ///
    /// A file with nothing to display used to render an empty black pane, which
    /// looks identical to a bug.
    #[must_use]
    pub fn empty_reason(&self) -> Option<EmptyDiffReason> {
        if !self.lines.is_empty() {
            return None;
        }
        Some(if self.is_binary {
            EmptyDiffReason::Binary
        } else if matches!(self.status, FileStatus::Renamed | FileStatus::Copied) {
            EmptyDiffReason::MovedWithoutChanges
        } else {
            EmptyDiffReason::NoLineChanges
        })
    }

    /// Generates a large deterministic fixture without reading a repository.
    #[must_use]
    pub fn demo(line_count: usize) -> Self {
        let mut lines = Vec::with_capacity(line_count);
        let mut old_line = 1_u32;
        let mut new_line = 1_u32;

        for index in 0..line_count {
            let (kind, old, new) = match index % 20 {
                5 => {
                    let line = old_line;
                    old_line += 1;
                    (DiffLineKind::Deletion, Some(line), None)
                }
                6 | 7 => {
                    let line = new_line;
                    new_line += 1;
                    (DiffLineKind::Addition, None, Some(line))
                }
                _ => {
                    let old = old_line;
                    let new = new_line;
                    old_line += 1;
                    new_line += 1;
                    (DiffLineKind::Context, Some(old), Some(new))
                }
            };

            let text: Arc<str> = match kind {
                DiffLineKind::Addition => format!(
                    "let reviewed_value_{index} = calculate_result(input, ReviewMode::Strict);"
                )
                .into(),
                DiffLineKind::Deletion => {
                    format!("let reviewed_value_{index} = calculate_result(input);").into()
                }
                DiffLineKind::Context => {
                    format!("assert_reviewable(reviewed_value_{index}, repository_guidance);")
                        .into()
                }
                DiffLineKind::NoNewlineMarker => unreachable!("demo lines always contain text"),
            };

            lines.push(DiffLine {
                kind,
                old_line: old,
                new_line: new,
                text,
            });
        }

        Self {
            path: "src/generated_review_fixture.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![DiffHunk {
                header: format!("@@ -1,{} +1,{} @@", old_line - 1, new_line - 1).into(),
                old_start: 1,
                new_start: 1,
                line_range: 0..line_count,
            }]
            .into(),
            counts: ChangeCounts::of(&lines),
            lines: lines.into(),
        }
    }
}

/// Why a reviewed file has no rows to show.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmptyDiffReason {
    /// Git reported the content as binary.
    Binary,
    /// Renamed or copied with identical content.
    MovedWithoutChanges,
    /// Something changed that is not line content, such as a file mode.
    NoLineChanges,
}

impl EmptyDiffReason {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Binary => "Binary file",
            Self::MovedWithoutChanges => "Moved without content changes",
            Self::NoLineChanges => "No line changes",
        }
    }

    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Binary => "ZReview does not render binary content yet.",
            Self::MovedWithoutChanges => {
                "The file's path changed but every line is identical, so there is nothing to diff."
            }
            Self::NoLineChanges => {
                "Something outside the file's line content changed, such as its mode."
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionSource {
    Demo,
    LocalComparison {
        repository_root: PathBuf,
        base_sha: Arc<str>,
        diff_base_sha: Arc<str>,
        head_sha: Arc<str>,
    },
    GitHubPullRequest {
        repository_root: PathBuf,
        owner: Arc<str>,
        repository: Arc<str>,
        number: u64,
        title: Arc<str>,
        url: Arc<str>,
        base_ref: Arc<str>,
        head_ref: Arc<str>,
        /// The base branch tip the comparison was taken against.
        base_sha: Arc<str>,
        /// GitHub's recorded `base.sha`, kept as provenance only. It is pinned
        /// when the PR is created or synchronized and drifts as the base branch
        /// advances, so it never defines the comparison.
        recorded_base_sha: Arc<str>,
        diff_base_sha: Arc<str>,
        head_sha: Arc<str>,
    },
}

impl SessionSource {
    /// The commit every anchor, finding, and draft in this session is keyed to.
    #[must_use]
    pub fn diff_base_sha(&self) -> Option<&Arc<str>> {
        match self {
            Self::Demo => None,
            Self::LocalComparison { diff_base_sha, .. }
            | Self::GitHubPullRequest { diff_base_sha, .. } => Some(diff_base_sha),
        }
    }

    /// The head commit under review.
    #[must_use]
    pub fn head_sha(&self) -> Option<&Arc<str>> {
        match self {
            Self::Demo => None,
            Self::LocalComparison { head_sha, .. } | Self::GitHubPullRequest { head_sha, .. } => {
                Some(head_sha)
            }
        }
    }

    /// The identity that persisted drafts belong to.
    ///
    /// Deliberately excludes the head commit. A pull request that is pushed to
    /// mid-review must still hand back the drafts written against the old head —
    /// as stale, needing re-anchoring, but present. Keying on the head instead
    /// would leave them in storage and invisible, which is losing them by another
    /// name.
    ///
    /// A pull request's scope is its repository and number, so drafts follow the
    /// review rather than the clone it was read in. A local comparison has no
    /// identity beyond its checkout, so it uses that.
    #[must_use]
    pub fn draft_scope(&self) -> Option<String> {
        match self {
            Self::Demo => None,
            Self::LocalComparison {
                repository_root, ..
            } => Some(format!("local:{}", repository_root.display())),
            Self::GitHubPullRequest {
                owner,
                repository,
                number,
                ..
            } => Some(format!("github:{owner}/{repository}#{number}")),
        }
    }
}

/// The two positions a re-anchored draft touched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReanchoredDraft {
    /// Where the draft used to claim to be, now empty.
    pub vacated: DiffAnchor,
    /// Where it is now, validated against the current diff.
    pub anchored: DiffAnchor,
}

/// The outcome of restoring drafts saved in an earlier session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RestoredDrafts {
    /// Drafts that still resolve to a displayed row.
    pub anchored: usize,
    /// Drafts kept but no longer attachable to a row.
    pub stale: usize,
}

impl RestoredDrafts {
    #[must_use]
    pub const fn total(self) -> usize {
        self.anchored + self.stale
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmptyReviewSession;

impl Display for EmptyReviewSession {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a review session requires at least one changed file")
    }
}

impl Error for EmptyReviewSession {}

/// UI-independent state for navigating one immutable comparison snapshot.
#[derive(Clone, Debug)]
pub struct ReviewSession {
    source: SessionSource,
    files: Arc<[DiffFile]>,
    selected_file: usize,
    viewed_paths: BTreeSet<Arc<str>>,
    anchors: Option<AnchorIndex>,
    comments: Arc<PlacedComments>,
    warnings: Vec<SessionFailure>,
    drafts: Arc<Drafts>,
    summary: String,
    /// Findings from the most recent review run, waiting to be acted on.
    findings: Findings,
    /// Claims the reviewer rejected, kept so a re-run does not offer them again.
    dismissed: DismissedFindings,
}

/// What happened when a reviewer accepted a finding.
#[derive(Clone, Debug, PartialEq)]
pub enum FindingAcceptance {
    /// Written as a draft. The anchor and body are returned so persistence can
    /// follow.
    Drafted { anchor: DiffAnchor, body: String },
    /// A draft is already at that anchor, so nothing was written.
    ///
    /// Both texts come back for a composer to open pre-filled: overwriting the
    /// reviewer's own words is the one thing acceptance must never do silently, and
    /// choosing between them is not a decision this layer should make.
    Occupied {
        anchor: DiffAnchor,
        location: AnchorLocation,
        existing: String,
        proposed: String,
    },
    /// The finding is about the change as a whole, so there is no line to attach it
    /// to. Its text is returned for the summary, if the reviewer wants it there.
    NotInline { proposed: String },
    /// No pending finding has that id.
    Unknown,
}

impl ReviewSession {
    /// Creates a session pinned to a fixed collection of changed files.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyReviewSession`] when `files` is empty.
    pub fn new(source: SessionSource, files: Arc<[DiffFile]>) -> Result<Self, EmptyReviewSession> {
        if files.is_empty() {
            return Err(EmptyReviewSession);
        }
        let anchors = source
            .head_sha()
            .map(|head_sha| AnchorIndex::new(&files, Arc::clone(head_sha)));
        Ok(Self {
            source,
            files,
            selected_file: 0,
            viewed_paths: BTreeSet::new(),
            anchors,
            comments: Arc::new(PlacedComments::default()),
            warnings: Vec::new(),
            drafts: Arc::new(Drafts::default()),
            summary: String::new(),
            findings: Findings::default(),
            dismissed: DismissedFindings::default(),
        })
    }

    /// The review body that accompanies the inline comments.
    ///
    /// GitHub requires one for a `COMMENT` or `REQUEST_CHANGES` review, and it is
    /// the only place a remark that belongs to no single line can go.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn set_summary(&mut self, summary: impl Into<String>) {
        self.summary = summary.into();
    }

    #[must_use]
    pub fn drafts(&self) -> &Drafts {
        &self.drafts
    }

    /// A snapshot of the drafts for a view to render.
    ///
    /// Mutations copy on write, so a view holding a snapshot keeps rendering a
    /// consistent set while the session moves on.
    #[must_use]
    pub fn shared_drafts(&self) -> Arc<Drafts> {
        Arc::clone(&self.drafts)
    }

    /// The anchor a displayed row would be commented on, if it can carry one.
    #[must_use]
    pub fn anchor_for(&self, file: usize, row: usize) -> Option<DiffAnchor> {
        let anchors = self.anchors.as_ref()?;
        anchors.anchor_for_row(self.files.get(file)?, row)
    }

    /// The anchor covering a span of displayed rows.
    ///
    /// Collapses to a single-line anchor when the span is one row, so an ordinary
    /// comment is never submitted as a one-line range.
    #[must_use]
    pub fn anchor_for_span(&self, file: usize, rows: RangeInclusive<usize>) -> Option<DiffAnchor> {
        let anchors = self.anchors.as_ref()?;
        anchors.anchor_for_rows(self.files.get(file)?, *rows.start(), *rows.end())
    }

    /// Creates or replaces the draft covering a span of displayed rows.
    ///
    /// The draft is keyed by the span's last line, which is where GitHub anchors a
    /// range and where it is drawn — so turning a single-line draft into a range
    /// edits the same draft rather than creating a second one beside it.
    ///
    /// Returns `false` when the span cannot carry a comment, including when its
    /// ends fall in different hunks.
    pub fn set_draft_over(
        &mut self,
        file: usize,
        rows: RangeInclusive<usize>,
        body: impl Into<String>,
    ) -> bool {
        let Some(anchor) = self.anchor_for_span(file, rows.clone()) else {
            return false;
        };
        // A range must be submittable, not merely constructible.
        if self
            .anchors
            .as_ref()
            .is_none_or(|anchors| anchors.resolve(&anchor).is_err())
        {
            return false;
        }

        let body = body.into();
        let end_row = *rows.end();
        let drafts = Arc::make_mut(&mut self.drafts);
        if body.trim().is_empty() {
            drafts.remove_at(file, end_row);
        } else {
            drafts.insert(anchor, body, file, end_row);
        }
        true
    }

    /// Creates or replaces the draft on a displayed row.
    ///
    /// An empty body removes the draft instead of storing a comment with nothing
    /// in it. Returns `false` when the row cannot carry a comment, which is the
    /// caller's signal not to offer a composer there at all.
    pub fn set_draft(&mut self, file: usize, row: usize, body: impl Into<String>) -> bool {
        self.set_draft_over(file, row..=row, body)
    }

    /// Discards the draft on a row, if there is one.
    pub fn clear_draft(&mut self, file: usize, row: usize) -> bool {
        Arc::make_mut(&mut self.drafts)
            .remove_at(file, row)
            .is_some()
    }

    /// Moves a stale draft onto a row in the current diff.
    ///
    /// A stale draft holds text that cannot be submitted, because the position it
    /// was written against is no longer in the diff. This is how it becomes
    /// submittable again: the reviewer picks a line that is, and the text moves
    /// there with a freshly validated anchor.
    ///
    /// Returns the anchors involved — the one vacated and the one now holding the
    /// text — so persistence can follow the move. `None` when the target row
    /// cannot carry a comment, or `stale` names no stale draft.
    pub fn reanchor_draft(
        &mut self,
        stale: &DiffAnchor,
        file: usize,
        row: usize,
    ) -> Option<ReanchoredDraft> {
        let target = self.anchor_for(file, row)?;
        // Checked before the text is removed, so a refused move changes nothing.
        if self.drafts.get(&target).is_some() {
            return None;
        }
        let body = Arc::make_mut(&mut self.drafts).take_stale(stale)?;
        Arc::make_mut(&mut self.drafts).insert(target.clone(), body, file, row);

        Some(ReanchoredDraft {
            vacated: stale.clone(),
            anchored: target,
        })
    }

    #[must_use]
    pub fn draft_at(&self, file: usize, row: usize) -> Option<&DraftComment> {
        self.drafts.at(file, row)
    }

    /// Findings waiting for the reviewer to accept or dismiss.
    #[must_use]
    pub const fn findings(&self) -> &Findings {
        &self.findings
    }

    /// Claims the reviewer has dismissed, so a re-run can suppress them.
    #[must_use]
    pub const fn dismissed_findings(&self) -> &DismissedFindings {
        &self.dismissed
    }

    /// Replaces the pending findings with the result of a review run.
    ///
    /// Anything the reviewer already dismissed is suppressed here rather than
    /// offered again, and the count of what was suppressed is returned so the run
    /// can say so — a review that hid twelve findings looks exactly like one that
    /// found nothing otherwise.
    pub fn set_findings(&mut self, mut findings: Findings) -> usize {
        let suppressed = findings.drop_dismissed(&self.dismissed);
        self.findings = findings;
        suppressed
    }

    /// Restores dismissals recorded in an earlier session.
    pub fn restore_dismissed(&mut self, dismissed: DismissedFindings) {
        self.dismissed = dismissed;
    }

    /// Reattaches provenance to drafts restored from storage.
    ///
    /// Called after [`restore_drafts`], because provenance describes a draft that
    /// must already be there. Provenance for an anchor holding no draft is ignored:
    /// the draft was submitted or discarded, and an attribution with nothing to
    /// attribute is not worth keeping.
    ///
    /// Returns how many drafts got their provenance back.
    ///
    /// [`restore_drafts`]: Self::restore_drafts
    pub fn restore_provenance(
        &mut self,
        provenance: Vec<(DiffAnchor, FindingProvenance)>,
    ) -> usize {
        let drafts = Arc::make_mut(&mut self.drafts);
        provenance
            .into_iter()
            .filter(|(anchor, recorded)| drafts.set_provenance(anchor, recorded.clone()))
            .count()
    }

    /// Accepts a finding, writing its proposed comment as a draft.
    ///
    /// This is the moment a finding stops being a finding. The reviewer read it and
    /// said yes, so it becomes an ordinary draft — persisted, re-anchored after a
    /// force-push, and submitted by exactly the same machinery as a comment they
    /// typed themselves. What it keeps is its provenance.
    ///
    /// Refuses to overwrite. One anchor holds one draft, so a finding landing where
    /// the reviewer has already written something would destroy their words, and
    /// this module does not do that. That case comes back as
    /// [`FindingAcceptance::Occupied`] carrying both texts, for a composer to open
    /// pre-filled so the reviewer decides.
    pub fn accept_finding(&mut self, id: FindingId) -> FindingAcceptance {
        let Some(finding) = self.findings.get(id) else {
            return FindingAcceptance::Unknown;
        };
        let (Some(anchor), Some(location)) = (finding.anchor.clone(), finding.location) else {
            return FindingAcceptance::NotInline {
                proposed: finding.proposed_comment.clone(),
            };
        };

        if let Some(existing) = self.drafts.get(&anchor) {
            return FindingAcceptance::Occupied {
                anchor,
                location,
                existing: existing.body.clone(),
                proposed: finding.proposed_comment.clone(),
            };
        }

        let provenance = finding.provenance();
        let body = finding.proposed_comment.clone();
        Arc::make_mut(&mut self.drafts).insert_with(
            anchor.clone(),
            body.clone(),
            Some(provenance),
            location.file,
            location.row,
        );
        // Only now, once the text is somewhere durable.
        self.findings.take(id);
        FindingAcceptance::Drafted { anchor, body }
    }

    /// Dismisses a finding and remembers the decision.
    ///
    /// Returns the fingerprint to persist, so the same claim is not offered again on
    /// the next run.
    pub fn dismiss_finding(&mut self, id: FindingId) -> Option<String> {
        let finding = self.findings.take(id)?;
        self.dismissed.insert(finding.fingerprint.clone());
        Some(finding.fingerprint)
    }

    /// Clears a finding the reviewer resolved by hand, after an
    /// [`FindingAcceptance::Occupied`] composer wrote the merged text.
    ///
    /// Separate from [`accept_finding`] because the draft already exists by then;
    /// this only retires the finding that prompted it.
    ///
    /// [`accept_finding`]: Self::accept_finding
    pub fn retire_finding(&mut self, id: FindingId) -> Option<Finding> {
        self.findings.take(id)
    }

    /// Assembles the review that submitting would post.
    ///
    /// Every draft is re-resolved against the snapshot as it is added, so a
    /// position the forge would reject cannot reach the request even if the draft
    /// was valid when it was written. Drafts that no longer resolve are returned
    /// as excluded rather than dropped, because a reviewer who is told nothing
    /// would believe they had submitted them.
    ///
    /// Building is separate from sending so the result can be shown to a human
    /// first. Nothing here contacts a forge.
    ///
    /// # Errors
    ///
    /// Returns [`SubmissionRefused`] when the session is not a pull request, there
    /// is nothing to say, or the event needs a summary that is missing.
    pub fn prepare_submission(
        &self,
        event: ReviewEvent,
    ) -> Result<ReviewSubmission, SubmissionRefused> {
        let (Some(head_sha), Some(anchors)) = (self.source.head_sha(), self.anchors.as_ref())
        else {
            return Err(SubmissionRefused::NotSubmittable);
        };

        let mut comments = Vec::new();
        let mut excluded = Vec::new();
        for draft in self.drafts.iter() {
            if anchors.resolve(&draft.anchor).is_ok() {
                comments.push(SubmittableComment {
                    path: Arc::clone(&draft.anchor.path),
                    side: draft.anchor.side,
                    line: draft.anchor.line,
                    // Only a genuine span becomes a range; a start equal to the
                    // end would make GitHub reject an otherwise valid comment.
                    start_line: draft
                        .anchor
                        .is_multiline()
                        .then_some(draft.anchor.start_line)
                        .flatten(),
                    body: draft.body.clone(),
                });
            } else {
                excluded.push(ExcludedDraft {
                    draft: draft.clone(),
                    reason: ExclusionReason::NotAnchored,
                });
            }
        }

        let body = self.summary.trim().to_owned();
        if comments.is_empty() && body.is_empty() {
            return Err(SubmissionRefused::Empty);
        }
        if event.requires_body() && body.is_empty() {
            return Err(SubmissionRefused::BodyRequired(event));
        }

        Ok(ReviewSubmission {
            head_sha: Arc::clone(head_sha),
            event,
            body,
            comments,
            excluded,
        })
    }

    /// Forgets the drafts a forge has accepted, and the summary that went with
    /// them.
    ///
    /// Called only after a successful response. Excluded drafts are deliberately
    /// left alone: they were not posted, so they are still the only copy.
    pub fn mark_submitted(&mut self, submission: &ReviewSubmission) {
        let drafts = Arc::make_mut(&mut self.drafts);
        for anchor in submission.submitted_anchors() {
            drafts.remove_anchored(&anchor);
        }
        self.summary.clear();
    }

    /// Restores drafts written in an earlier session, reporting how many could no
    /// longer be anchored.
    ///
    /// A draft whose anchor no longer resolves is kept and marked stale rather
    /// than dropped: the diff can change under a draft — a base branch that moved
    /// shifts the merge base, and with it which lines are displayed — and silently
    /// deleting the reviewer's words would be the worst possible response.
    pub fn restore_drafts(
        &mut self,
        drafts: impl IntoIterator<Item = (DiffAnchor, String)>,
    ) -> RestoredDrafts {
        let mut restored = RestoredDrafts::default();
        for (anchor, body) in drafts {
            let location = self
                .anchors
                .as_ref()
                .and_then(|index| index.resolve(&anchor).ok());
            if let Some(location) = location {
                Arc::make_mut(&mut self.drafts).insert(anchor, body, location.file, location.row);
                restored.anchored += 1;
            } else {
                Arc::make_mut(&mut self.drafts).insert_stale(anchor, body);
                restored.stale += 1;
            }
        }
        restored
    }

    /// Records something that went wrong without making the session unusable.
    ///
    /// Conversations that would not load, or drafts that cannot be persisted, do
    /// not stop a reviewer reading the diff — but they must be told. A pull
    /// request that silently appears to have no discussion is worse than one that
    /// says its discussion is missing, and a reviewer typing into something that
    /// is not saving needs to know immediately.
    pub fn push_warning(&mut self, warning: SessionFailure) {
        self.warnings.push(warning);
    }

    #[must_use]
    pub fn warnings(&self) -> &[SessionFailure] {
        &self.warnings
    }

    /// Places published review comments against this snapshot and reports how
    /// many threads resulted.
    ///
    /// A session with no head commit cannot anchor comments and places none, so a
    /// zero return is the caller's signal that nothing was shown.
    pub fn set_review_comments(&mut self, comments: Vec<ReviewComment>) -> usize {
        let placed = self
            .anchors
            .as_ref()
            .map_or_else(PlacedComments::default, |anchors| {
                PlacedComments::new(comments, anchors)
            });
        let count = placed.thread_count();
        self.comments = Arc::new(placed);
        count
    }

    #[must_use]
    pub fn comments(&self) -> &PlacedComments {
        &self.comments
    }

    #[must_use]
    pub fn shared_comments(&self) -> Arc<PlacedComments> {
        Arc::clone(&self.comments)
    }

    /// The anchor index for this snapshot.
    ///
    /// Absent for sources with no head commit, which therefore cannot carry
    /// review comments.
    #[must_use]
    pub fn anchors(&self) -> Option<&AnchorIndex> {
        self.anchors.as_ref()
    }

    #[must_use]
    pub const fn source(&self) -> &SessionSource {
        &self.source
    }

    #[must_use]
    pub fn files(&self) -> &[DiffFile] {
        &self.files
    }

    #[must_use]
    pub fn shared_files(&self) -> Arc<[DiffFile]> {
        self.files.clone()
    }

    #[must_use]
    pub const fn selected_file_index(&self) -> usize {
        self.selected_file
    }

    #[must_use]
    pub fn selected_file(&self) -> &DiffFile {
        &self.files[self.selected_file]
    }

    /// Selects a file by index and reports whether the selection changed.
    pub fn select_file(&mut self, index: usize) -> bool {
        if index >= self.files.len() || index == self.selected_file {
            return false;
        }
        self.selected_file = index;
        true
    }

    pub fn select_next_file(&mut self) -> bool {
        let next = self
            .selected_file
            .saturating_add(1)
            .min(self.files.len().saturating_sub(1));
        self.select_file(next)
    }

    pub fn select_previous_file(&mut self) -> bool {
        self.select_file(self.selected_file.saturating_sub(1))
    }

    pub fn toggle_selected_viewed(&mut self) -> bool {
        let path = self.selected_file().path.clone();
        if self.viewed_paths.remove(&path) {
            false
        } else {
            self.viewed_paths.insert(path);
            true
        }
    }

    #[must_use]
    pub fn is_viewed(&self, index: usize) -> bool {
        self.files
            .get(index)
            .is_some_and(|file| self.viewed_paths.contains(&file.path))
    }

    #[must_use]
    pub fn viewed_count(&self) -> usize {
        self.viewed_paths.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_has_stable_line_coordinates() {
        let file = DiffFile::demo(100_000);

        assert_eq!(file.line_count(), 100_000);
        assert_eq!(file.hunks[0].line_range, 0..100_000);
        assert_eq!(file.lines[5].kind, DiffLineKind::Deletion);
        assert!(file.lines[5].old_line.is_some());
        assert_eq!(file.lines[5].new_line, None);
        assert_eq!(file.lines[6].kind, DiffLineKind::Addition);
        assert_eq!(file.lines[6].old_line, None);
        assert!(file.lines[6].new_line.is_some());
    }

    /// A multi-hunk file used to show only the first hunk's header, pinned above
    /// the whole scroll range.
    #[test]
    fn each_hunk_header_belongs_to_the_row_that_starts_it() {
        let file = DiffFile {
            path: "src/review.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![
                DiffHunk {
                    header: "@@ -10,2 +10,2 @@".into(),
                    old_start: 10,
                    new_start: 10,
                    line_range: 0..2,
                },
                DiffHunk {
                    header: "@@ -80,3 +80,3 @@".into(),
                    old_start: 80,
                    new_start: 80,
                    line_range: 2..5,
                },
            ]
            .into(),
            counts: ChangeCounts::default(),
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    old_line: Some(10),
                    new_line: Some(10),
                    text: "a".into(),
                };
                5
            ]
            .into(),
        };

        assert_eq!(
            file.hunk_header_at(0).map(ToString::to_string),
            Some("@@ -10,2 +10,2 @@".to_owned()),
        );
        assert!(file.hunk_header_at(1).is_none(), "only the starting row");
        assert_eq!(
            file.hunk_header_at(2).map(ToString::to_string),
            Some("@@ -80,3 +80,3 @@".to_owned()),
        );
        assert!(file.hunk_header_at(9).is_none());

        // Every row still knows which hunk it is in.
        assert_eq!(file.hunk_at(1).unwrap().old_start, 10);
        assert_eq!(file.hunk_at(4).unwrap().old_start, 80);
        assert!(file.hunk_at(5).is_none(), "past the last hunk");
    }

    #[test]
    fn change_counts_are_computed_once_from_the_lines() {
        let file = DiffFile::demo(20);
        let recounted = ChangeCounts::of(&file.lines);

        assert_eq!(file.counts, recounted);
        // The fixture repeats a deletion and two additions every twenty lines.
        assert_eq!(file.counts.deletions, 1);
        assert_eq!(file.counts.additions, 2);
    }

    #[test]
    fn a_file_with_rows_has_no_empty_reason() {
        assert!(DiffFile::demo(10).empty_reason().is_none());
    }

    /// Each of these used to render an identical empty black pane.
    #[test]
    fn files_with_nothing_to_show_say_why() {
        let empty = |status, is_binary| DiffFile {
            path: "image.bin".into(),
            old_path: None,
            status,
            is_binary,
            hunks: Arc::from([]),
            counts: ChangeCounts::default(),
            lines: Arc::from([]),
        };

        assert_eq!(
            empty(FileStatus::Modified, true).empty_reason(),
            Some(EmptyDiffReason::Binary),
        );
        assert_eq!(
            empty(FileStatus::Renamed, false).empty_reason(),
            Some(EmptyDiffReason::MovedWithoutChanges),
        );
        assert_eq!(
            empty(FileStatus::Modified, false).empty_reason(),
            Some(EmptyDiffReason::NoLineChanges),
        );

        for reason in [
            EmptyDiffReason::Binary,
            EmptyDiffReason::MovedWithoutChanges,
            EmptyDiffReason::NoLineChanges,
        ] {
            assert!(!reason.label().is_empty());
            assert!(!reason.detail().is_empty());
        }
    }

    #[test]
    fn marker_matches_line_kind() {
        assert_eq!(DiffLineKind::Context.marker(), ' ');
        assert_eq!(DiffLineKind::Addition.marker(), '+');
        assert_eq!(DiffLineKind::Deletion.marker(), '-');
        assert_eq!(DiffLineKind::NoNewlineMarker.marker(), '\\');
    }

    #[test]
    fn session_navigates_and_tracks_viewed_files() {
        let mut first = DiffFile::demo(10);
        first.path = "src/first.rs".into();
        let mut second = DiffFile::demo(20);
        second.path = "src/second.rs".into();
        let mut session =
            ReviewSession::new(SessionSource::Demo, vec![first, second].into()).unwrap();

        assert_eq!(session.selected_file().path.as_ref(), "src/first.rs");
        assert!(session.toggle_selected_viewed());
        assert!(session.is_viewed(0));
        assert_eq!(session.viewed_count(), 1);
        assert!(session.select_next_file());
        assert_eq!(session.selected_file().path.as_ref(), "src/second.rs");
        assert!(!session.select_next_file());
        assert!(session.select_previous_file());
        assert!(!session.toggle_selected_viewed());
        assert_eq!(session.viewed_count(), 0);
    }

    #[test]
    fn only_repository_backed_sources_can_be_anchored() {
        assert!(SessionSource::Demo.diff_base_sha().is_none());
        assert!(SessionSource::Demo.head_sha().is_none());

        let local = SessionSource::LocalComparison {
            repository_root: PathBuf::from("/tmp/repository"),
            base_sha: "b".repeat(40).into(),
            diff_base_sha: "d".repeat(40).into(),
            head_sha: "h".repeat(40).into(),
        };
        assert_eq!(local.diff_base_sha().unwrap().as_ref(), "d".repeat(40));
        assert_eq!(local.head_sha().unwrap().as_ref(), "h".repeat(40));
    }

    #[test]
    fn a_repository_backed_session_anchors_its_rows() {
        let head_sha = "h".repeat(40);
        let source = SessionSource::LocalComparison {
            repository_root: PathBuf::from("/tmp/repository"),
            base_sha: "b".repeat(40).into(),
            diff_base_sha: "d".repeat(40).into(),
            head_sha: head_sha.clone().into(),
        };
        let mut file = DiffFile::demo(40);
        file.path = "src/review.rs".into();
        let session = ReviewSession::new(source, vec![file].into()).unwrap();

        let anchors = session.anchors().expect("a local comparison has a head");
        assert_eq!(anchors.head_sha().as_ref(), head_sha);

        let anchor = anchors
            .anchor_for_row(session.selected_file(), 6)
            .expect("row 6 is an addition");
        assert_eq!(anchor.side, DiffSide::Right);
        assert_eq!(anchor.path.as_ref(), "src/review.rs");
        assert_eq!(anchors.resolve(&anchor).unwrap().row, 6);
    }

    fn anchored_session() -> ReviewSession {
        let source = SessionSource::LocalComparison {
            repository_root: PathBuf::from("/tmp/repository"),
            base_sha: "b".repeat(40).into(),
            diff_base_sha: "d".repeat(40).into(),
            head_sha: "h".repeat(40).into(),
        };
        let mut file = DiffFile::demo(40);
        file.path = "src/review.rs".into();
        ReviewSession::new(source, vec![file].into()).unwrap()
    }

    /// A raw finding on a row the demo fixture makes commentable.
    ///
    /// Row 6 of the fixture is an addition, so it anchors right at its new line.
    fn raw_finding(session: &ReviewSession, row: usize, title: &str) -> RawFinding {
        let anchor = session
            .anchor_for(0, row)
            .expect("the row can carry a comment");
        RawFinding {
            location: Some(RawLocation {
                path: anchor.path,
                side: anchor.side,
                line: anchor.line,
                start_line: None,
            }),
            severity: Severity::Warning,
            confidence: 0.9,
            title: title.to_owned(),
            rationale: "because".to_owned(),
            proposed_comment: "Handle the failure here.".to_owned(),
            guidance_sources: vec![GuidanceCitation {
                path: "AGENTS.md".into(),
                content_hash: "hash".into(),
            }],
        }
    }

    fn with_findings(session: &mut ReviewSession, raw: Vec<RawFinding>) {
        let anchors = session.anchors().expect("the session is anchored").clone();
        let findings = Findings::validate(raw, &anchors, &FindingOrigin::Ai("claude-code".into()));
        session.set_findings(findings);
    }

    #[test]
    fn accepting_a_finding_writes_it_as_a_draft_carrying_its_provenance() {
        let mut session = anchored_session();
        let raw = raw_finding(&session, 6, "unchecked index");
        with_findings(&mut session, vec![raw]);
        let id = session.findings().accepted()[0].id;

        let outcome = session.accept_finding(id);

        let FindingAcceptance::Drafted { anchor, body } = outcome else {
            panic!("expected the finding to become a draft, got {outcome:?}");
        };
        assert_eq!(body, "Handle the failure here.");
        let draft = session.drafts().get(&anchor).expect("the draft is there");
        assert!(draft.is_proposed());
        let provenance = draft.provenance.as_ref().expect("provenance is recorded");
        assert_eq!(provenance.origin, FindingOrigin::Ai("claude-code".into()));
        assert_eq!(provenance.guidance_sources[0].path.as_ref(), "AGENTS.md");
        // Acted on, so no longer pending.
        assert!(session.findings().is_empty());
    }

    #[test]
    fn a_draft_the_reviewer_wrote_has_no_provenance() {
        let mut session = anchored_session();
        session.set_draft(0, 6, "my own thought");

        let draft = session.draft_at(0, 6).expect("the draft is there");

        assert!(!draft.is_proposed());
    }

    /// The case that decided the design: acceptance must never overwrite.
    #[test]
    fn accepting_onto_a_line_the_reviewer_already_commented_on_writes_nothing() {
        let mut session = anchored_session();
        session.set_draft(0, 6, "I already said something here");
        let raw = raw_finding(&session, 6, "unchecked index");
        with_findings(&mut session, vec![raw]);
        let id = session.findings().accepted()[0].id;

        let outcome = session.accept_finding(id);

        let FindingAcceptance::Occupied {
            location,
            existing,
            proposed,
            ..
        } = outcome
        else {
            panic!("expected the anchor to be occupied, got {outcome:?}");
        };
        assert_eq!(existing, "I already said something here");
        assert_eq!(proposed, "Handle the failure here.");
        assert_eq!(location.row, 6);
        // The reviewer's words are untouched, and the finding is still pending.
        assert_eq!(
            session.draft_at(0, 6).map(|draft| draft.body.as_str()),
            Some("I already said something here")
        );
        assert_eq!(session.findings().len(), 1);
    }

    #[test]
    fn resolving_an_occupied_anchor_by_hand_retires_the_finding() {
        let mut session = anchored_session();
        session.set_draft(0, 6, "mine");
        let raw = raw_finding(&session, 6, "unchecked index");
        with_findings(&mut session, vec![raw]);
        let id = session.findings().accepted()[0].id;

        // What the composer does once the reviewer merges the two texts.
        assert!(session.set_draft(0, 6, "mine, and also: handle the failure"));
        let retired = session.retire_finding(id);

        assert!(retired.is_some());
        assert!(session.findings().is_empty());
        assert_eq!(
            session.draft_at(0, 6).map(|draft| draft.body.as_str()),
            Some("mine, and also: handle the failure")
        );
    }

    #[test]
    fn rewording_an_accepted_finding_keeps_its_provenance() {
        let mut session = anchored_session();
        let raw = raw_finding(&session, 6, "unchecked index");
        with_findings(&mut session, vec![raw]);
        let id = session.findings().accepted()[0].id;
        session.accept_finding(id);

        // The reviewer edits the wording; it is still a suggestion they took.
        assert!(session.set_draft(0, 6, "Please handle this failure case."));

        let draft = session.draft_at(0, 6).expect("the draft is there");
        assert_eq!(draft.body, "Please handle this failure case.");
        assert!(draft.is_proposed());
    }

    #[test]
    fn a_finding_about_the_whole_change_has_no_line_to_attach_to() {
        let mut session = anchored_session();
        let mut raw = raw_finding(&session, 6, "no tests anywhere");
        raw.location = None;
        with_findings(&mut session, vec![raw]);
        let id = session.findings().accepted()[0].id;

        let outcome = session.accept_finding(id);

        assert!(matches!(outcome, FindingAcceptance::NotInline { .. }));
        assert!(session.drafts().is_empty());
    }

    #[test]
    fn dismissing_a_finding_records_it_so_a_re_run_does_not_offer_it_again() {
        let mut session = anchored_session();
        let raw = raw_finding(&session, 6, "unchecked index");
        with_findings(&mut session, vec![raw.clone()]);
        let id = session.findings().accepted()[0].id;

        let fingerprint = session
            .dismiss_finding(id)
            .expect("dismissing returns the fingerprint to persist");

        assert!(session.findings().is_empty());
        assert!(session.dismissed_findings().contains(&fingerprint));

        // The same claim on a second run is suppressed, and says so.
        with_findings(&mut session, vec![raw]);
        assert!(session.findings().is_empty());
        assert!(matches!(
            session.findings().rejected()[0].reason,
            RejectionReason::PreviouslyDismissed { .. }
        ));
    }

    #[test]
    fn a_dismissal_does_not_suppress_a_different_claim_on_the_same_line() {
        let mut session = anchored_session();
        let dismissed = raw_finding(&session, 6, "unchecked index");
        let other = raw_finding(&session, 6, "misleading name");
        with_findings(&mut session, vec![dismissed]);
        let id = session.findings().accepted()[0].id;
        session.dismiss_finding(id);

        with_findings(&mut session, vec![other]);

        assert_eq!(session.findings().len(), 1);
        assert_eq!(session.findings().accepted()[0].title, "misleading name");
    }

    #[test]
    fn set_findings_reports_how_many_it_suppressed() {
        let mut session = anchored_session();
        let first = raw_finding(&session, 6, "one");
        let second = raw_finding(&session, 7, "two");
        with_findings(&mut session, vec![first.clone(), second.clone()]);
        let id = session.findings().accepted()[0].id;
        session.dismiss_finding(id);

        let anchors = session.anchors().expect("anchored").clone();
        let findings = Findings::validate(
            vec![first, second],
            &anchors,
            &FindingOrigin::Ai("claude-code".into()),
        );
        let suppressed = session.set_findings(findings);

        assert_eq!(suppressed, 1);
        assert_eq!(session.findings().len(), 1);
    }

    #[test]
    fn dismissals_restored_from_an_earlier_session_still_suppress() {
        let mut session = anchored_session();
        let raw = raw_finding(&session, 6, "unchecked index");
        let anchors = session.anchors().expect("anchored").clone();
        let findings = Findings::validate(
            vec![raw],
            &anchors,
            &FindingOrigin::Ai("claude-code".into()),
        );
        let fingerprint = findings.accepted()[0].fingerprint.clone();

        session.restore_dismissed([fingerprint].into_iter().collect());
        let suppressed = session.set_findings(findings);

        assert_eq!(suppressed, 1);
        assert!(session.findings().is_empty());
    }

    #[test]
    fn accepting_a_finding_that_is_not_pending_does_nothing() {
        let mut session = anchored_session();

        let outcome = session.accept_finding(FindingId(7));

        assert_eq!(outcome, FindingAcceptance::Unknown);
        assert!(session.drafts().is_empty());
    }

    #[test]
    fn an_accepted_finding_submits_like_any_other_draft() {
        let mut session = anchored_session();
        let raw = raw_finding(&session, 6, "unchecked index");
        with_findings(&mut session, vec![raw]);
        let id = session.findings().accepted()[0].id;
        session.accept_finding(id);
        session.set_summary("Took one suggestion from the review.");

        let submission = session
            .prepare_submission(ReviewEvent::Comment)
            .expect("the review has a comment to post");

        assert_eq!(submission.comments.len(), 1);
        assert_eq!(submission.comments[0].body, "Handle the failure here.");
        assert!(submission.excluded.is_empty());
    }

    /// A pull request keeps its drafts across a push; the head is not in the key.
    #[test]
    fn draft_scope_identifies_the_review_not_the_head() {
        let pull_request = SessionSource::GitHubPullRequest {
            repository_root: PathBuf::from("/tmp/repository"),
            owner: "acme".into(),
            repository: "widgets".into(),
            number: 42,
            title: "Improve the review flow".into(),
            url: "https://github.com/acme/widgets/pull/42".into(),
            base_ref: "main".into(),
            head_ref: "feature".into(),
            base_sha: "b".repeat(40).into(),
            recorded_base_sha: "r".repeat(40).into(),
            diff_base_sha: "d".repeat(40).into(),
            head_sha: "h".repeat(40).into(),
        };
        assert_eq!(
            pull_request.draft_scope().unwrap(),
            "github:acme/widgets#42",
        );

        let local = SessionSource::LocalComparison {
            repository_root: PathBuf::from("/tmp/repository"),
            base_sha: "b".repeat(40).into(),
            diff_base_sha: "d".repeat(40).into(),
            head_sha: "h".repeat(40).into(),
        };
        assert_eq!(local.draft_scope().unwrap(), "local:/tmp/repository");

        // Nothing to persist for a generated fixture.
        assert!(SessionSource::Demo.draft_scope().is_none());
    }

    #[test]
    fn a_draft_is_created_against_the_rows_anchor() {
        let mut session = anchored_session();

        // Row 6 of the fixture is an addition, so it anchors on the right.
        assert!(session.set_draft(0, 6, "needs a test"));

        let draft = session.draft_at(0, 6).expect("the draft should be stored");
        assert_eq!(draft.body, "needs a test");
        assert_eq!(draft.anchor.side, DiffSide::Right);
        assert_eq!(draft.anchor.path.as_ref(), "src/review.rs");
        assert_eq!(draft.anchor.head_sha.as_ref(), "h".repeat(40));
        assert!(!draft.is_stale);
        assert_eq!(session.drafts().len(), 1);
    }

    #[test]
    fn an_emptied_draft_is_removed_rather_than_stored_blank() {
        let mut session = anchored_session();
        session.set_draft(0, 6, "a thought");

        assert!(session.set_draft(0, 6, "   \n  "));

        assert!(session.draft_at(0, 6).is_none());
        assert!(session.drafts().is_empty());
    }

    #[test]
    fn clearing_a_draft_reports_whether_there_was_one() {
        let mut session = anchored_session();
        session.set_draft(0, 6, "a thought");

        assert!(session.clear_draft(0, 6));
        assert!(!session.clear_draft(0, 6));
    }

    #[test]
    fn a_session_without_anchors_cannot_hold_drafts() {
        let mut session =
            ReviewSession::new(SessionSource::Demo, vec![DiffFile::demo(20)].into()).unwrap();

        assert!(
            !session.set_draft(0, 6, "nowhere to put this"),
            "a demo session has no head to anchor against",
        );
        assert!(session.drafts().is_empty());
    }

    #[test]
    fn a_row_that_cannot_carry_a_comment_is_refused() {
        let source = SessionSource::LocalComparison {
            repository_root: PathBuf::from("/tmp/repository"),
            base_sha: "b".repeat(40).into(),
            diff_base_sha: "d".repeat(40).into(),
            head_sha: "h".repeat(40).into(),
        };
        let mut file = DiffFile::demo(4);
        file.path = "src/review.rs".into();
        // Replace the last row with a marker, which has no source line.
        let mut lines = file.lines.to_vec();
        lines[3] = DiffLine {
            kind: DiffLineKind::NoNewlineMarker,
            old_line: None,
            new_line: None,
            text: "No newline at end of file".into(),
        };
        file.lines = lines.into();
        let mut session = ReviewSession::new(source, vec![file].into()).unwrap();

        assert!(!session.set_draft(0, 3, "cannot go here"));
        assert!(session.set_draft(0, 0, "but this can"));
        assert_eq!(session.drafts().len(), 1);
    }

    #[test]
    fn restored_drafts_reattach_to_their_rows() {
        let mut session = anchored_session();
        let anchor = session.anchor_for(0, 6).unwrap();

        let restored = session.restore_drafts([(anchor, "from last time".to_owned())]);

        assert_eq!(restored.anchored, 1);
        assert_eq!(restored.stale, 0);
        assert_eq!(restored.total(), 1);
        assert_eq!(session.draft_at(0, 6).unwrap().body, "from last time");
    }

    /// The diff can change under a draft, so restoring must keep text it can no
    /// longer place rather than deleting the reviewer's words.
    #[test]
    fn a_draft_that_no_longer_resolves_is_kept_as_stale() {
        let mut session = anchored_session();
        let unreachable = DiffAnchor {
            path: "src/review.rs".into(),
            side: DiffSide::Right,
            line: 9_999,
            start_line: None,
            head_sha: "h".repeat(40).into(),
        };

        let restored = session.restore_drafts([(unreachable, "still worth saying".to_owned())]);

        assert_eq!(restored.anchored, 0);
        assert_eq!(restored.stale, 1);
        assert_eq!(session.drafts().len(), 1);
        assert_eq!(session.drafts().stale_count(), 1);
        assert_eq!(
            session.drafts().stale().next().unwrap().body,
            "still worth saying",
        );
    }

    #[test]
    fn a_draft_from_another_snapshot_is_kept_as_stale() {
        let mut session = anchored_session();
        let mut anchor = session.anchor_for(0, 6).unwrap();
        anchor.head_sha = "0".repeat(40).into();

        let restored =
            session.restore_drafts([(anchor, "written against an older head".to_owned())]);

        assert_eq!(restored.stale, 1);
        assert_eq!(session.drafts().stale_count(), 1);
        // It did not take the row it would have occupied.
        assert!(session.draft_at(0, 6).is_none());
    }

    fn stale_anchor() -> DiffAnchor {
        DiffAnchor {
            path: "src/review.rs".into(),
            side: DiffSide::Right,
            line: 9_999,
            start_line: None,
            head_sha: "h".repeat(40).into(),
        }
    }

    /// Without this, a kept stale draft is text the reviewer can see and never
    /// submit.
    #[test]
    fn a_stale_draft_can_be_moved_onto_a_line_in_the_diff() {
        let mut session = anchored_session();
        session.restore_drafts([(stale_anchor(), "still worth saying".to_owned())]);
        assert_eq!(session.drafts().stale_count(), 1);

        let moved = session
            .reanchor_draft(&stale_anchor(), 0, 6)
            .expect("row 6 can carry a comment");

        assert_eq!(moved.vacated, stale_anchor());
        assert_eq!(moved.anchored.line, 6);
        assert_eq!(moved.anchored.side, DiffSide::Right);

        // The text is now anchored where it can be submitted.
        let draft = session.draft_at(0, 6).expect("it should be on the row");
        assert_eq!(draft.body, "still worth saying");
        assert!(!draft.is_stale);
        assert_eq!(session.drafts().stale_count(), 0);
        assert_eq!(session.drafts().len(), 1, "moved, not duplicated");
    }

    #[test]
    fn re_anchoring_refuses_a_row_that_cannot_carry_a_comment() {
        let source = SessionSource::LocalComparison {
            repository_root: PathBuf::from("/tmp/repository"),
            base_sha: "b".repeat(40).into(),
            diff_base_sha: "d".repeat(40).into(),
            head_sha: "h".repeat(40).into(),
        };
        let mut file = DiffFile::demo(4);
        file.path = "src/review.rs".into();
        let mut lines = file.lines.to_vec();
        lines[3] = DiffLine {
            kind: DiffLineKind::NoNewlineMarker,
            old_line: None,
            new_line: None,
            text: "No newline at end of file".into(),
        };
        file.lines = lines.into();
        let mut session = ReviewSession::new(source, vec![file].into()).unwrap();
        session.restore_drafts([(stale_anchor(), "still worth saying".to_owned())]);

        assert!(session.reanchor_draft(&stale_anchor(), 0, 3).is_none());
        // Refused without consuming the text.
        assert_eq!(session.drafts().stale_count(), 1);
        assert_eq!(
            session.drafts().stale().next().unwrap().body,
            "still worth saying",
        );
    }

    /// One anchor holds one draft, so a move must not silently overwrite whatever
    /// is already on the target row.
    #[test]
    fn re_anchoring_refuses_a_row_that_already_has_a_draft() {
        let mut session = anchored_session();
        session.set_draft(0, 6, "written just now");
        session.restore_drafts([(stale_anchor(), "written last week".to_owned())]);

        assert!(session.reanchor_draft(&stale_anchor(), 0, 6).is_none());

        assert_eq!(session.draft_at(0, 6).unwrap().body, "written just now");
        assert_eq!(
            session.drafts().stale().next().unwrap().body,
            "written last week",
        );
    }

    #[test]
    fn re_anchoring_an_anchor_with_no_stale_draft_does_nothing() {
        let mut session = anchored_session();
        session.set_draft(0, 0, "an ordinary draft");
        let anchored = session.anchor_for(0, 0).unwrap();

        // Not stale, so it cannot be pulled out by anchor.
        assert!(session.reanchor_draft(&anchored, 0, 6).is_none());
        assert!(session.reanchor_draft(&stale_anchor(), 0, 6).is_none());

        assert_eq!(session.draft_at(0, 0).unwrap().body, "an ordinary draft");
        assert_eq!(session.drafts().len(), 1);
    }

    #[test]
    fn a_submission_carries_every_anchored_draft_and_the_summary() {
        let mut session = anchored_session();
        // Row 5 of the fixture is a deletion and row 6 an addition, so these are
        // the same line number on opposite sides.
        session.set_draft(0, 5, "why was this removed?");
        session.set_draft(0, 6, "needs a test");
        session.set_summary("  Close, two notes.  ");

        let submission = session.prepare_submission(ReviewEvent::Comment).unwrap();

        assert_eq!(submission.event, ReviewEvent::Comment);
        assert_eq!(submission.head_sha.as_ref(), "h".repeat(40));
        // Trimmed, since a body of whitespace is not a body.
        assert_eq!(submission.body, "Close, two notes.");
        // In reading order, which is how they will be posted.
        assert_eq!(
            submission
                .comments
                .iter()
                .map(|comment| (comment.line, comment.side, comment.body.as_str()))
                .collect::<Vec<_>>(),
            [
                (6, DiffSide::Left, "why was this removed?"),
                (6, DiffSide::Right, "needs a test"),
            ],
        );
        assert!(submission.excluded.is_empty());
    }

    /// A stale draft must never be silently posted at a position the forge would
    /// reject, and must never be silently dropped either.
    #[test]
    fn a_stale_draft_is_excluded_from_the_submission_and_reported() {
        let mut session = anchored_session();
        session.set_draft(0, 6, "will be posted");
        session.restore_drafts([(stale_anchor(), "will not be posted".to_owned())]);
        session.set_summary("Some notes.");

        let submission = session.prepare_submission(ReviewEvent::Comment).unwrap();

        assert_eq!(submission.comments.len(), 1);
        assert_eq!(submission.comments[0].body, "will be posted");
        assert_eq!(submission.excluded.len(), 1);
        assert_eq!(submission.excluded[0].draft.body, "will not be posted");
        assert_eq!(submission.excluded[0].reason, ExclusionReason::NotAnchored);
    }

    #[test]
    fn an_approval_needs_no_summary_but_the_other_events_do() {
        let mut session = anchored_session();
        session.set_draft(0, 6, "one note");

        // GitHub accepts an approval with no body.
        assert!(session.prepare_submission(ReviewEvent::Approve).is_ok());

        assert_eq!(
            session
                .prepare_submission(ReviewEvent::Comment)
                .unwrap_err(),
            SubmissionRefused::BodyRequired(ReviewEvent::Comment),
        );
        assert_eq!(
            session
                .prepare_submission(ReviewEvent::RequestChanges)
                .unwrap_err(),
            SubmissionRefused::BodyRequired(ReviewEvent::RequestChanges),
        );

        session.set_summary("Two things.");
        assert!(session.prepare_submission(ReviewEvent::Comment).is_ok());
    }

    #[test]
    fn a_review_with_nothing_in_it_is_refused() {
        let session = anchored_session();

        for event in [
            ReviewEvent::Comment,
            ReviewEvent::Approve,
            ReviewEvent::RequestChanges,
        ] {
            assert_eq!(
                session.prepare_submission(event).unwrap_err(),
                SubmissionRefused::Empty,
                "an empty review should be refused for {event}",
            );
        }
    }

    /// A submission whose only drafts are stale would post nothing inline, so it
    /// still needs a summary to be worth sending.
    #[test]
    fn a_submission_of_only_stale_drafts_needs_a_summary() {
        let mut session = anchored_session();
        session.restore_drafts([(stale_anchor(), "cannot be posted".to_owned())]);

        assert_eq!(
            session
                .prepare_submission(ReviewEvent::Approve)
                .unwrap_err(),
            SubmissionRefused::Empty,
        );

        session.set_summary("Approving despite the stale note.");
        let submission = session.prepare_submission(ReviewEvent::Approve).unwrap();
        assert!(submission.comments.is_empty());
        assert_eq!(submission.excluded.len(), 1);
    }

    #[test]
    fn a_session_that_is_not_a_pull_request_cannot_be_submitted() {
        let mut session =
            ReviewSession::new(SessionSource::Demo, vec![DiffFile::demo(20)].into()).unwrap();
        session.set_summary("Nowhere to send this.");

        assert_eq!(
            session
                .prepare_submission(ReviewEvent::Comment)
                .unwrap_err(),
            SubmissionRefused::NotSubmittable,
        );
    }

    /// Only what was actually posted is forgotten. An excluded draft is still the
    /// only copy of its text.
    #[test]
    fn marking_submitted_forgets_the_posted_drafts_and_keeps_the_rest() {
        let mut session = anchored_session();
        session.set_draft(0, 6, "posted");
        session.restore_drafts([(stale_anchor(), "not posted".to_owned())]);
        session.set_summary("A summary.");
        let submission = session.prepare_submission(ReviewEvent::Comment).unwrap();

        session.mark_submitted(&submission);

        assert!(session.draft_at(0, 6).is_none(), "the posted draft is gone");
        assert_eq!(session.drafts().len(), 1, "the stale one remains");
        assert_eq!(session.drafts().stale_count(), 1);
        assert_eq!(session.summary(), "", "the summary went with the review");
    }

    /// A range draft has to survive the whole path: created over a span, keyed at
    /// its last row, and submitted with both range fields.
    #[test]
    fn a_range_draft_is_created_keyed_and_submitted_as_one_comment() {
        let mut session = anchored_session();

        // Rows 0..=4 of the fixture are context lines in one hunk, so the span is
        // a right-side range.
        assert!(session.set_draft_over(0, 0..=4, "this whole block needs a test"));

        let draft = session
            .draft_at(0, 4)
            .expect("a range is keyed at its last row");
        assert!(draft.anchor.is_multiline());
        assert_eq!(draft.anchor.start_line, Some(1));
        assert_eq!(draft.anchor.line, 5);
        assert_eq!(session.drafts().len(), 1);

        session.set_summary("One note.");
        let submission = session.prepare_submission(ReviewEvent::Comment).unwrap();
        assert_eq!(submission.comments.len(), 1);
        assert_eq!(submission.comments[0].start_line, Some(1));
        assert_eq!(submission.comments[0].line, 5);
    }

    /// Extending a selection over a line that already has a comment edits that
    /// comment rather than creating a rival beside it.
    #[test]
    fn widening_a_draft_into_a_range_replaces_it() {
        let mut session = anchored_session();
        session.set_draft(0, 4, "just this line");

        assert!(session.set_draft_over(0, 0..=4, "actually this whole block"));

        assert_eq!(session.drafts().len(), 1, "one draft, now a range");
        let draft = session.draft_at(0, 4).unwrap();
        assert_eq!(draft.body, "actually this whole block");
        assert!(draft.anchor.is_multiline());
    }

    #[test]
    fn a_single_row_span_stays_a_single_line_draft() {
        let mut session = anchored_session();
        session.set_draft_over(0, 6..=6, "one line");

        let draft = session.draft_at(0, 6).unwrap();
        assert_eq!(draft.anchor.start_line, None);
        assert!(!draft.anchor.is_multiline());
    }

    /// A span whose ends fall in different hunks is refused, not silently
    /// truncated to something submittable.
    #[test]
    fn a_span_crossing_hunks_is_refused() {
        let source = SessionSource::LocalComparison {
            repository_root: PathBuf::from("/tmp/repository"),
            base_sha: "b".repeat(40).into(),
            diff_base_sha: "d".repeat(40).into(),
            head_sha: "h".repeat(40).into(),
        };
        let mut file = DiffFile::demo(8);
        file.path = "src/review.rs".into();
        // Split the file into two hunks so a span can cross them.
        file.hunks = vec![
            DiffHunk {
                header: "@@ -1,4 +1,4 @@".into(),
                old_start: 1,
                new_start: 1,
                line_range: 0..4,
            },
            DiffHunk {
                header: "@@ -40,4 +40,4 @@".into(),
                old_start: 40,
                new_start: 40,
                line_range: 4..8,
            },
        ]
        .into();
        let mut session = ReviewSession::new(source, vec![file].into()).unwrap();

        assert!(
            !session.set_draft_over(0, 0..=4, "spans two hunks"),
            "a span crossing hunks cannot be submitted, so it is refused",
        );
        assert!(session.drafts().is_empty());
        // Either hunk on its own is fine.
        assert!(session.set_draft_over(0, 0..=3, "within the first hunk"));
    }

    #[test]
    fn a_demo_session_has_no_anchor_index() {
        let session = ReviewSession::new(SessionSource::Demo, vec![DiffFile::demo(10)].into())
            .expect("the demo has files");
        assert!(session.anchors().is_none());
    }

    #[test]
    fn session_rejects_an_empty_file_collection() {
        assert!(ReviewSession::new(SessionSource::Demo, Arc::from([])).is_err());
    }
}
