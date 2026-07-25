//! Mapping between displayed diff rows and GitHub review-comment positions.
//!
//! GitHub anchors an inline review comment with a `path`, a `side`, and a `line`
//! number on that side. Nothing may become an inline comment until it has been
//! resolved against the snapshot it claims to belong to, so this module owns both
//! directions of the mapping and the validation gate.

use std::{
    collections::HashMap,
    error::Error,
    fmt::{Display, Formatter},
    sync::Arc,
};

use crate::{DiffFile, DiffLineKind};

/// Which revision of a file a comment is attached to.
///
/// Ordered so a draft queue can be sorted by position: the base revision's view
/// of a line comes before the head's.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DiffSide {
    /// The base revision. Carries deletion and context lines.
    Left,
    /// The head revision. Carries addition and context lines.
    Right,
}

impl DiffSide {
    /// The value GitHub's review-comment API expects.
    #[must_use]
    pub const fn github_value(self) -> &'static str {
        match self {
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
        }
    }

    /// Parses a `side` field from a GitHub review comment.
    ///
    /// GitHub omits `side` for comments on the head revision, so a missing value
    /// is treated as [`DiffSide::Right`] by the caller rather than here.
    #[must_use]
    pub fn from_github(value: &str) -> Option<Self> {
        match value {
            "LEFT" => Some(Self::Left),
            "RIGHT" => Some(Self::Right),
            _ => None,
        }
    }
}

impl Display for DiffSide {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.github_value())
    }
}

/// A GitHub-submittable position in a reviewed file, pinned to one snapshot.
///
/// `path` is always the file's path at head, which is what GitHub expects even
/// for [`DiffSide::Left`] comments on a renamed file.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DiffAnchor {
    pub path: Arc<str>,
    pub side: DiffSide,
    /// 1-based line number on `side`. For a range, its last line — which is where
    /// GitHub anchors the comment and where it is drawn.
    pub line: u32,
    /// First line of a multi-line range.
    ///
    /// Always on the same side as `line` and never greater than it. Ranges that
    /// straddle both sides are not offered: GitHub allows a separate `start_side`,
    /// but a comment whose start and end are on different revisions is far easier
    /// to create by accident than to mean.
    pub start_line: Option<u32>,
    /// The head commit this anchor was created against. An anchor from an
    /// earlier head must never be silently reused against a newer one.
    pub head_sha: Arc<str>,
}

impl DiffAnchor {
    /// An anchor on one line.
    #[must_use]
    pub fn single(path: Arc<str>, side: DiffSide, line: u32, head_sha: Arc<str>) -> Self {
        Self {
            path,
            side,
            line,
            start_line: None,
            head_sha,
        }
    }

    /// Whether this anchor covers more than one line.
    ///
    /// A range whose start equals its end is a single line, not a one-line range,
    /// so it reports `false` and submits without range fields.
    #[must_use]
    pub fn is_multiline(&self) -> bool {
        self.start_line.is_some_and(|start| start < self.line)
    }

    /// The lines this anchor covers, first to last.
    #[must_use]
    pub fn line_span(&self) -> std::ops::RangeInclusive<u32> {
        self.start_line.unwrap_or(self.line).min(self.line)..=self.line
    }
}

/// Where a resolved anchor lands in the rendered session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnchorLocation {
    /// Index into the session's files.
    pub file: usize,
    /// Index into that file's flattened diff lines.
    pub row: usize,
}

/// Why an anchor cannot be used against a snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnchorError {
    /// The anchor was created against a different head commit.
    StaleSnapshot { expected: Arc<str>, found: Arc<str> },
    /// No file in the snapshot has this path.
    UnknownPath(Arc<str>),
    /// The file is under review, but that line is not displayed on that side.
    LineNotInDiff {
        path: Arc<str>,
        side: DiffSide,
        line: u32,
    },
    /// A range that ends before it starts.
    InvertedRange { start_line: u32, line: u32 },
    /// A range whose ends fall in different hunks, so the lines between them are
    /// not all part of the diff.
    RangeCrossesHunks {
        path: Arc<str>,
        start_line: u32,
        line: u32,
    },
}

impl Display for AnchorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleSnapshot { expected, found } => write!(
                formatter,
                "the comment was written against {found} but the snapshot is at {expected}"
            ),
            Self::UnknownPath(path) => {
                write!(formatter, "{path} is not part of this review")
            }
            Self::LineNotInDiff { path, side, line } => write!(
                formatter,
                "{path} {side} line {line} is not a displayed diff line"
            ),
            Self::InvertedRange { start_line, line } => write!(
                formatter,
                "a comment range cannot start at line {start_line} and end at {line}"
            ),
            Self::RangeCrossesHunks {
                path,
                start_line,
                line,
            } => write!(
                formatter,
                "{path} lines {start_line} to {line} span more than one hunk, so the lines between them are not all in the diff"
            ),
        }
    }
}

impl Error for AnchorError {}

#[derive(Clone, Debug)]
struct FileAnchors {
    file: usize,
    /// Base-revision line number to displayed row.
    left: HashMap<u32, usize>,
    /// Head-revision line number to displayed row.
    right: HashMap<u32, usize>,
    /// Which hunk each displayed row belongs to, so a range can be checked for
    /// staying inside one.
    hunk_of_row: Vec<usize>,
}

/// Bidirectional map between displayed diff rows and GitHub anchors, for exactly
/// one snapshot.
///
/// Line numbers are unique per side within a file because the parser assigns
/// them monotonically across hunks, so each side maps one line to one row.
#[derive(Clone, Debug)]
pub struct AnchorIndex {
    head_sha: Arc<str>,
    files: HashMap<Arc<str>, FileAnchors>,
}

impl AnchorIndex {
    /// Indexes every commentable line in a snapshot's files.
    #[must_use]
    pub fn new(files: &[DiffFile], head_sha: Arc<str>) -> Self {
        let mut indexed = HashMap::with_capacity(files.len());
        for (file_index, file) in files.iter().enumerate() {
            let mut anchors = FileAnchors {
                file: file_index,
                left: HashMap::new(),
                right: HashMap::new(),
                hunk_of_row: vec![usize::MAX; file.lines.len()],
            };
            for (hunk_index, hunk) in file.hunks.iter().enumerate() {
                for row in hunk.line_range.clone() {
                    if let Some(slot) = anchors.hunk_of_row.get_mut(row) {
                        *slot = hunk_index;
                    }
                }
            }
            for (row, line) in file.lines.iter().enumerate() {
                if let Some(old_line) = line.old_line {
                    anchors.left.insert(old_line, row);
                }
                if let Some(new_line) = line.new_line {
                    anchors.right.insert(new_line, row);
                }
            }
            indexed.insert(file.path.clone(), anchors);
        }

        Self {
            head_sha,
            files: indexed,
        }
    }

    /// The head commit every anchor in this index is pinned to.
    #[must_use]
    pub const fn head_sha(&self) -> &Arc<str> {
        &self.head_sha
    }

    /// The session file index for a reviewed path.
    #[must_use]
    pub fn file_index(&self, path: &str) -> Option<usize> {
        self.files.get(path).map(|file| file.file)
    }

    /// The number of commentable positions across the snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files
            .values()
            .map(|file| file.left.len() + file.right.len())
            .sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The anchor GitHub expects for a displayed row.
    ///
    /// Additions exist only on the right and deletions only on the left. Context
    /// lines exist on both sides and are anchored on the right, matching what
    /// GitHub's own review UI submits. Rows that carry no source line, such as
    /// the missing-final-newline marker, cannot be commented on.
    #[must_use]
    pub fn anchor_for_row(&self, file: &DiffFile, row: usize) -> Option<DiffAnchor> {
        let line = file.line(row)?;
        let (side, number) = match line.kind {
            DiffLineKind::Addition | DiffLineKind::Context => (DiffSide::Right, line.new_line?),
            DiffLineKind::Deletion => (DiffSide::Left, line.old_line?),
            DiffLineKind::NoNewlineMarker => return None,
        };

        Some(DiffAnchor::single(
            file.path.clone(),
            side,
            number,
            Arc::clone(&self.head_sha),
        ))
    }

    /// Validates an anchor against this snapshot and locates its displayed row.
    ///
    /// This is the gate every inline comment must pass: an anchor that does not
    /// resolve cannot be submitted to GitHub, whether it came from a reviewer, a
    /// review backend, or an earlier session.
    ///
    /// # Errors
    ///
    /// Returns [`AnchorError`] when the anchor belongs to another snapshot, names
    /// a path that is not under review, or points at a line that is not
    /// displayed on the requested side.
    pub fn resolve(&self, anchor: &DiffAnchor) -> Result<AnchorLocation, AnchorError> {
        if anchor.head_sha != self.head_sha {
            return Err(AnchorError::StaleSnapshot {
                expected: Arc::clone(&self.head_sha),
                found: Arc::clone(&anchor.head_sha),
            });
        }

        let file = self
            .files
            .get(&anchor.path)
            .ok_or_else(|| AnchorError::UnknownPath(Arc::clone(&anchor.path)))?;
        let rows = match anchor.side {
            DiffSide::Left => &file.left,
            DiffSide::Right => &file.right,
        };

        let row = *rows
            .get(&anchor.line)
            .ok_or_else(|| AnchorError::LineNotInDiff {
                path: Arc::clone(&anchor.path),
                side: anchor.side,
                line: anchor.line,
            })?;

        // A range must start no later than it ends, and both ends must sit in the
        // same hunk. Line numbers run contiguously within a hunk on each side, so
        // matching hunks is enough to know every line between them is displayed
        // too — which is what GitHub requires of a range.
        if let Some(start_line) = anchor.start_line
            && start_line != anchor.line
        {
            if start_line > anchor.line {
                return Err(AnchorError::InvertedRange {
                    start_line,
                    line: anchor.line,
                });
            }
            let start_row = *rows
                .get(&start_line)
                .ok_or_else(|| AnchorError::LineNotInDiff {
                    path: Arc::clone(&anchor.path),
                    side: anchor.side,
                    line: start_line,
                })?;
            if file.hunk_of_row.get(start_row) != file.hunk_of_row.get(row) {
                return Err(AnchorError::RangeCrossesHunks {
                    path: Arc::clone(&anchor.path),
                    start_line,
                    line: anchor.line,
                });
            }
        }

        Ok(AnchorLocation {
            file: file.file,
            row,
        })
    }

    /// The anchor covering two displayed rows, ordered so the earlier row starts
    /// the range.
    ///
    /// Returns `None` unless both rows can carry a comment on the same side, which
    /// is what makes a range submittable.
    #[must_use]
    pub fn anchor_for_rows(
        &self,
        file: &DiffFile,
        first_row: usize,
        second_row: usize,
    ) -> Option<DiffAnchor> {
        let (start_row, end_row) = if first_row <= second_row {
            (first_row, second_row)
        } else {
            (second_row, first_row)
        };
        let end = self.anchor_for_row(file, end_row)?;
        if start_row == end_row {
            return Some(end);
        }

        // Both ends must anchor on the same side; a range across revisions is not
        // offered.
        let start = self.anchor_for_row(file, start_row)?;
        if start.side != end.side {
            return None;
        }
        Some(DiffAnchor {
            start_line: Some(start.line),
            ..end
        })
    }

    /// Whether this anchor may become an inline GitHub review comment.
    #[must_use]
    pub fn is_valid(&self, anchor: &DiffAnchor) -> bool {
        self.resolve(anchor).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiffHunk, DiffLine, FileStatus};

    const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER_HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn line(kind: DiffLineKind, old_line: Option<u32>, new_line: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            old_line,
            new_line,
            text: "source".into(),
        }
    }

    /// Two hunks with a gap between them, so lines that exist in the file but are
    /// not displayed can be told apart from lines that are.
    fn two_hunk_file() -> DiffFile {
        let lines = vec![
            // Hunk one, old/new lines 10..=12.
            line(DiffLineKind::Context, Some(10), Some(10)),
            line(DiffLineKind::Deletion, Some(11), None),
            line(DiffLineKind::Addition, None, Some(11)),
            line(DiffLineKind::Context, Some(12), Some(12)),
            // Hunk two, well past the gap.
            line(DiffLineKind::Context, Some(80), Some(80)),
            line(DiffLineKind::Addition, None, Some(81)),
            line(DiffLineKind::NoNewlineMarker, None, None),
        ];

        DiffFile {
            path: "src/review.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![
                DiffHunk {
                    header: "@@ -10,3 +10,3 @@".into(),
                    old_start: 10,
                    new_start: 10,
                    line_range: 0..4,
                },
                DiffHunk {
                    header: "@@ -80,1 +80,2 @@".into(),
                    old_start: 80,
                    new_start: 80,
                    line_range: 4..7,
                },
            ]
            .into(),
            counts: crate::ChangeCounts::of(&lines),
            lines: lines.into(),
        }
    }

    fn index_for(files: &[DiffFile]) -> AnchorIndex {
        AnchorIndex::new(files, HEAD.into())
    }

    #[test]
    fn anchor_side_follows_the_line_kind() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        let context = index.anchor_for_row(&file, 0).unwrap();
        assert_eq!(context.side, DiffSide::Right);
        assert_eq!(context.line, 10);

        let deletion = index.anchor_for_row(&file, 1).unwrap();
        assert_eq!(deletion.side, DiffSide::Left);
        assert_eq!(deletion.line, 11);

        let addition = index.anchor_for_row(&file, 2).unwrap();
        assert_eq!(addition.side, DiffSide::Right);
        assert_eq!(addition.line, 11);

        // The missing-final-newline marker is not a source line.
        assert!(index.anchor_for_row(&file, 6).is_none());
        // Neither is a row past the end of the file.
        assert!(index.anchor_for_row(&file, 999).is_none());
    }

    #[test]
    fn every_commentable_row_round_trips_through_its_anchor() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        let mut anchored_rows = 0;
        for row in 0..file.line_count() {
            let Some(anchor) = index.anchor_for_row(&file, row) else {
                continue;
            };
            assert_eq!(anchor.head_sha.as_ref(), HEAD);
            assert_eq!(
                index.resolve(&anchor).unwrap(),
                AnchorLocation { file: 0, row },
                "row {row} did not resolve back to itself"
            );
            anchored_rows += 1;
        }

        // Every row except the marker.
        assert_eq!(anchored_rows, file.line_count() - 1);
    }

    #[test]
    fn deleted_and_added_lines_resolve_only_on_their_own_side() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        // Old line 11 was deleted, so it exists on the left but not the right.
        assert!(index.is_valid(&DiffAnchor {
            path: "src/review.rs".into(),
            side: DiffSide::Left,
            line: 11,
            start_line: None,
            head_sha: HEAD.into(),
        }));

        // New line 81 was added, so it exists on the right but not the left.
        assert!(index.is_valid(&DiffAnchor {
            path: "src/review.rs".into(),
            side: DiffSide::Right,
            line: 81,
            start_line: None,
            head_sha: HEAD.into(),
        }));
        let error = index
            .resolve(&DiffAnchor {
                path: "src/review.rs".into(),
                side: DiffSide::Left,
                line: 81,
                start_line: None,
                head_sha: HEAD.into(),
            })
            .unwrap_err();
        assert_eq!(
            error,
            AnchorError::LineNotInDiff {
                path: "src/review.rs".into(),
                side: DiffSide::Left,
                line: 81,
            }
        );
    }

    #[test]
    fn context_lines_resolve_on_both_sides() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        for side in [DiffSide::Left, DiffSide::Right] {
            assert!(
                index.is_valid(&DiffAnchor {
                    path: "src/review.rs".into(),
                    side,
                    line: 10,
                    start_line: None,
                    head_sha: HEAD.into(),
                }),
                "context line should be anchorable on {side}"
            );
        }
    }

    #[test]
    fn lines_outside_a_displayed_hunk_are_rejected() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        // Line 40 exists in the file but falls in the gap between the hunks, so
        // GitHub would reject a comment on it.
        let error = index
            .resolve(&DiffAnchor {
                path: "src/review.rs".into(),
                side: DiffSide::Right,
                line: 40,
                start_line: None,
                head_sha: HEAD.into(),
            })
            .unwrap_err();
        assert!(matches!(error, AnchorError::LineNotInDiff { line: 40, .. }));
        assert!(
            error.to_string().contains("not a displayed diff line"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn anchors_from_another_snapshot_are_rejected() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        let error = index
            .resolve(&DiffAnchor {
                path: "src/review.rs".into(),
                side: DiffSide::Right,
                line: 10,
                start_line: None,
                head_sha: OTHER_HEAD.into(),
            })
            .unwrap_err();

        assert_eq!(
            error,
            AnchorError::StaleSnapshot {
                expected: HEAD.into(),
                found: OTHER_HEAD.into(),
            }
        );
    }

    #[test]
    fn paths_outside_the_review_are_rejected() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        let error = index
            .resolve(&DiffAnchor {
                path: "src/untouched.rs".into(),
                side: DiffSide::Right,
                line: 10,
                start_line: None,
                head_sha: HEAD.into(),
            })
            .unwrap_err();

        assert_eq!(error, AnchorError::UnknownPath("src/untouched.rs".into()));
    }

    #[test]
    fn anchors_carry_the_file_index_they_belong_to() {
        let mut first = two_hunk_file();
        first.path = "src/first.rs".into();
        let mut second = two_hunk_file();
        second.path = "src/second.rs".into();
        let files = vec![first, second];
        let index = index_for(&files);

        let anchor = index.anchor_for_row(&files[1], 0).unwrap();
        assert_eq!(anchor.path.as_ref(), "src/second.rs");
        assert_eq!(index.resolve(&anchor).unwrap().file, 1);
    }

    #[test]
    fn binary_files_carry_no_anchors() {
        let binary = DiffFile {
            path: "image.bin".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: true,
            hunks: Arc::from([]),
            counts: crate::ChangeCounts::default(),
            lines: Arc::from([]),
        };
        let index = index_for(std::slice::from_ref(&binary));

        assert!(index.is_empty());
        assert!(index.anchor_for_row(&binary, 0).is_none());
        let error = index
            .resolve(&DiffAnchor {
                path: "image.bin".into(),
                side: DiffSide::Right,
                line: 1,
                start_line: None,
                head_sha: HEAD.into(),
            })
            .unwrap_err();
        // The file is under review; it simply has no commentable line.
        assert!(matches!(error, AnchorError::LineNotInDiff { .. }));
    }

    #[test]
    fn a_range_within_one_hunk_resolves_to_its_last_row() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        // Rows 0..=3 are the first hunk: context 10, deletion 11, addition 11,
        // context 12. A right-side range over the context lines spans 10 to 12.
        let anchor = index.anchor_for_rows(&file, 0, 3).unwrap();
        assert_eq!(anchor.side, DiffSide::Right);
        assert_eq!(anchor.start_line, Some(10));
        assert_eq!(anchor.line, 12);
        assert!(anchor.is_multiline());
        assert_eq!(anchor.line_span().collect::<Vec<_>>(), [10, 11, 12]);

        // It anchors where GitHub anchors it: the last line.
        assert_eq!(
            index.resolve(&anchor).unwrap(),
            AnchorLocation { file: 0, row: 3 },
        );
    }

    /// Selecting upwards must produce the same range as selecting downwards.
    #[test]
    fn a_range_is_ordered_however_it_was_selected() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        assert_eq!(
            index.anchor_for_rows(&file, 3, 0),
            index.anchor_for_rows(&file, 0, 3),
        );
    }

    /// A one-row span is an ordinary comment, not a one-line range: sending
    /// `start_line == line` would have GitHub reject it.
    #[test]
    fn a_single_row_span_is_not_a_range() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        let anchor = index.anchor_for_rows(&file, 0, 0).unwrap();
        assert_eq!(anchor.start_line, None);
        assert!(!anchor.is_multiline());
    }

    /// The lines between the ends have to be in the diff too, which is only
    /// guaranteed inside a single hunk.
    #[test]
    fn a_range_across_two_hunks_is_rejected() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        // Row 0 is in the first hunk, row 4 in the second.
        let anchor = index.anchor_for_rows(&file, 0, 4).unwrap();
        let error = index.resolve(&anchor).unwrap_err();

        assert!(
            matches!(
                error,
                AnchorError::RangeCrossesHunks {
                    start_line: 10,
                    line: 80,
                    ..
                }
            ),
            "unexpected error: {error:?}",
        );
        assert!(error.to_string().contains("more than one hunk"));
    }

    /// A deletion anchors left and an addition right, so a span covering both has
    /// no single side to submit against.
    #[test]
    fn a_range_across_both_sides_is_not_offered() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        // Row 1 is a deletion (left) and row 2 an addition (right).
        assert!(index.anchor_for_rows(&file, 1, 2).is_none());
    }

    #[test]
    fn a_range_cannot_end_before_it_starts() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        let error = index
            .resolve(&DiffAnchor {
                path: "src/review.rs".into(),
                side: DiffSide::Right,
                line: 10,
                start_line: Some(12),
                head_sha: HEAD.into(),
            })
            .unwrap_err();

        assert_eq!(
            error,
            AnchorError::InvertedRange {
                start_line: 12,
                line: 10,
            },
        );
    }

    #[test]
    fn a_range_starting_outside_the_diff_is_rejected() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        let error = index
            .resolve(&DiffAnchor {
                path: "src/review.rs".into(),
                side: DiffSide::Right,
                line: 12,
                // Line 5 is before the first hunk.
                start_line: Some(5),
                head_sha: HEAD.into(),
            })
            .unwrap_err();

        assert!(matches!(error, AnchorError::LineNotInDiff { line: 5, .. }));
    }

    /// A range spanning a row that cannot carry a comment still resolves, because
    /// its own ends are valid and the lines between are inside the hunk.
    #[test]
    fn a_range_may_cover_rows_that_are_not_themselves_anchorable() {
        let file = two_hunk_file();
        let index = index_for(std::slice::from_ref(&file));

        // Rows 4..=5 in the second hunk, which also contains a marker row at 6.
        let anchor = index.anchor_for_rows(&file, 4, 5).unwrap();
        assert_eq!(anchor.start_line, Some(80));
        assert_eq!(anchor.line, 81);
        assert!(index.resolve(&anchor).is_ok());
    }

    #[test]
    fn github_side_values_round_trip() {
        for side in [DiffSide::Left, DiffSide::Right] {
            assert_eq!(DiffSide::from_github(side.github_value()), Some(side));
        }
        assert_eq!(DiffSide::from_github("LEFT"), Some(DiffSide::Left));
        assert_eq!(DiffSide::from_github("RIGHT"), Some(DiffSide::Right));
        assert_eq!(DiffSide::from_github("right"), None);
        assert_eq!(DiffSide::from_github(""), None);
    }
}
