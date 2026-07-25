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

pub use anchor::{AnchorError, AnchorIndex, AnchorLocation, DiffAnchor, DiffSide};
pub use comment::{CommentThread, PlacedComments, ReviewComment, UnplacedReason, UnplacedThread};

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

/// A reviewable file. Lines are flat to make virtualized indexing constant-time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffFile {
    pub path: Arc<str>,
    pub old_path: Option<Arc<str>>,
    pub status: FileStatus,
    pub is_binary: bool,
    pub hunks: Arc<[DiffHunk]>,
    pub lines: Arc<[DiffLine]>,
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
            lines: lines.into(),
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
        })
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
