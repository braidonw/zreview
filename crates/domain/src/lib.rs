use std::{
    collections::BTreeSet,
    error::Error,
    fmt::{Display, Formatter},
    ops::Range,
    path::PathBuf,
    sync::Arc,
};

mod anchor;
mod comment;
mod draft;
mod session;

pub use anchor::{AnchorError, AnchorIndex, AnchorLocation, DiffAnchor, DiffSide};
pub use comment::{CommentThread, PlacedComments, ReviewComment, UnplacedReason, UnplacedThread};
pub use draft::{DraftComment, DraftSink, Drafts};
pub use session::{LoadStage, LoadedSession, SessionFailure};

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
        })
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

    /// Creates or replaces the draft on a displayed row.
    ///
    /// An empty body removes the draft instead of storing a comment with nothing
    /// in it. Returns `false` when the row cannot carry a comment, which is the
    /// caller's signal not to offer a composer there at all.
    pub fn set_draft(&mut self, file: usize, row: usize, body: impl Into<String>) -> bool {
        let Some(anchor) = self.anchor_for(file, row) else {
            return false;
        };
        let body = body.into();
        let drafts = Arc::make_mut(&mut self.drafts);
        if body.trim().is_empty() {
            drafts.remove_at(file, row);
        } else {
            drafts.insert(anchor, body, file, row);
        }
        true
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
