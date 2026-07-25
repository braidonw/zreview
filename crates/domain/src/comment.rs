//! Existing review comments, grouped into threads and placed against a snapshot.
//!
//! Comments arrive from a forge as a flat list. Rendering them needs two things
//! this module provides: replies collapsed into threads, and each thread resolved
//! to the displayed row it belongs to. A comment that cannot be placed is never
//! dropped — an outdated or file-level comment is still part of the conversation
//! a reviewer needs to read.

use std::{
    collections::HashMap,
    fmt::{Display, Formatter},
    sync::Arc,
};

use crate::{AnchorIndex, DiffAnchor, DiffSide};

/// One published review comment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewComment {
    pub id: u64,
    pub author: Arc<str>,
    pub body: Arc<str>,
    /// The file's path at head, as the forge reports it.
    pub path: Arc<str>,
    pub side: DiffSide,
    /// Line on `side` in the current head.
    ///
    /// Absent when the forge considers the comment outdated, meaning the line it
    /// was written against is no longer in the diff, or when the comment applies
    /// to the file as a whole.
    pub line: Option<u32>,
    /// First line of a multi-line comment's range. The comment is displayed at
    /// `line`, which is where the forge anchors it.
    pub start_line: Option<u32>,
    /// The comment this one replies to, if any.
    pub in_reply_to_id: Option<u64>,
    /// Whether the comment addresses the file as a whole rather than a line.
    /// File-level comments have no `line` but are not outdated.
    pub is_file_level: bool,
    pub created_at: Arc<str>,
    pub url: Arc<str>,
}

impl ReviewComment {
    /// The anchor this comment claims against a snapshot head.
    ///
    /// Returns `None` for comments with no line, which can never be anchored to
    /// a row. A returned anchor is a *claim*: it still has to be resolved
    /// against an [`AnchorIndex`] before it can be trusted.
    #[must_use]
    pub fn claimed_anchor(&self, head_sha: &Arc<str>) -> Option<DiffAnchor> {
        Some(DiffAnchor {
            path: Arc::clone(&self.path),
            side: self.side,
            line: self.line?,
            head_sha: Arc::clone(head_sha),
        })
    }

    /// Whether the comment spans more than one line.
    #[must_use]
    pub fn is_multiline(&self) -> bool {
        matches!((self.start_line, self.line), (Some(start), Some(end)) if start < end)
    }
}

/// A comment and its replies, in chronological order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentThread {
    comments: Vec<ReviewComment>,
}

impl CommentThread {
    /// The comment that opened the thread and determines where it is displayed.
    #[must_use]
    pub fn root(&self) -> &ReviewComment {
        &self.comments[0]
    }

    #[must_use]
    pub fn id(&self) -> u64 {
        self.root().id
    }

    #[must_use]
    pub fn comments(&self) -> &[ReviewComment] {
        &self.comments
    }

    /// The number of replies after the opening comment.
    #[must_use]
    pub fn reply_count(&self) -> usize {
        self.comments.len() - 1
    }
}

/// Why a thread could not be attached to a displayed row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnplacedReason {
    /// The comment addresses the whole file, so it never had a line.
    FileLevel,
    /// The forge dropped the line, meaning the code it was written against is no
    /// longer in the diff.
    Outdated,
    /// The line is not displayed in the current diff on that side.
    NotInDiff,
    /// The file is not part of this review at all.
    PathNotReviewed,
}

impl Display for UnplacedReason {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::FileLevel => "on the whole file",
            Self::Outdated => "outdated",
            Self::NotInDiff => "outside the diff",
            Self::PathNotReviewed => "on a file not under review",
        })
    }
}

/// A thread that is visible but not attached to a row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnplacedThread {
    pub thread: CommentThread,
    pub reason: UnplacedReason,
    /// The reviewed file the thread belongs to, when its path is under review.
    pub file: Option<usize>,
}

/// Existing threads resolved against one snapshot.
///
/// Row lookup is by `(file, row)` so a virtualized list can ask per visible row
/// without scanning.
#[derive(Clone, Debug, Default)]
pub struct PlacedComments {
    by_row: HashMap<(usize, usize), Vec<CommentThread>>,
    unplaced: Vec<UnplacedThread>,
}

impl PlacedComments {
    /// Groups comments into threads and resolves each thread against the index.
    #[must_use]
    pub fn new(comments: Vec<ReviewComment>, anchors: &AnchorIndex) -> Self {
        let mut placed = Self::default();

        for thread in group_into_threads(comments) {
            let root = thread.root();
            let Some(anchor) = root.claimed_anchor(anchors.head_sha()) else {
                let file = anchors.file_index(&root.path);
                let reason = if root.is_file_level {
                    UnplacedReason::FileLevel
                } else {
                    UnplacedReason::Outdated
                };
                placed.unplaced.push(UnplacedThread {
                    thread,
                    reason,
                    file,
                });
                continue;
            };

            if let Ok(location) = anchors.resolve(&anchor) {
                placed
                    .by_row
                    .entry((location.file, location.row))
                    .or_default()
                    .push(thread);
            } else {
                let file = anchors.file_index(&root.path);
                let reason = if file.is_some() {
                    UnplacedReason::NotInDiff
                } else {
                    UnplacedReason::PathNotReviewed
                };
                placed.unplaced.push(UnplacedThread {
                    thread,
                    reason,
                    file,
                });
            }
        }

        // Several threads can share a row; keep them in a stable published order.
        for threads in placed.by_row.values_mut() {
            threads.sort_by_key(CommentThread::id);
        }
        placed.unplaced.sort_by_key(|unplaced| unplaced.thread.id());
        placed
    }

    /// Threads anchored to a displayed row.
    #[must_use]
    pub fn threads_at(&self, file: usize, row: usize) -> &[CommentThread] {
        self.by_row
            .get(&(file, row))
            .map_or(&[], |threads| threads.as_slice())
    }

    /// Whether any thread is anchored to a displayed row.
    #[must_use]
    pub fn has_threads_at(&self, file: usize, row: usize) -> bool {
        self.by_row.contains_key(&(file, row))
    }

    /// Threads that are visible but not attached to a row.
    #[must_use]
    pub fn unplaced(&self) -> &[UnplacedThread] {
        &self.unplaced
    }

    /// Unplaced threads belonging to one reviewed file.
    pub fn unplaced_for_file(&self, file: usize) -> impl Iterator<Item = &UnplacedThread> {
        self.unplaced
            .iter()
            .filter(move |unplaced| unplaced.file == Some(file))
    }

    /// The number of anchored threads on a file.
    #[must_use]
    pub fn thread_count_for_file(&self, file: usize) -> usize {
        self.by_row
            .iter()
            .filter(|((candidate, _), _)| *candidate == file)
            .map(|(_, threads)| threads.len())
            .sum()
    }

    /// Every thread, anchored or not.
    #[must_use]
    pub fn thread_count(&self) -> usize {
        self.by_row.values().map(Vec::len).sum::<usize>() + self.unplaced.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.thread_count() == 0
    }
}

/// Collapses replies into threads.
///
/// A reply names the comment it answers, which is usually the thread's opening
/// comment but may be another reply, so parents are followed to the root. A
/// reply whose parent was not returned — it can be missing from a filtered or
/// partial response — starts its own thread rather than being discarded.
fn group_into_threads(mut comments: Vec<ReviewComment>) -> Vec<CommentThread> {
    comments.sort_by_key(|comment| comment.id);

    let parents = comments
        .iter()
        .map(|comment| (comment.id, comment.in_reply_to_id))
        .collect::<HashMap<_, _>>();
    let roots = thread_roots(&parents);

    let mut threads: Vec<CommentThread> = Vec::new();
    let mut thread_indices: HashMap<u64, usize> = HashMap::new();
    for comment in comments {
        let root_id = roots.get(&comment.id).copied().unwrap_or(comment.id);
        if let Some(index) = thread_indices.get(&root_id) {
            threads[*index].comments.push(comment);
        } else {
            thread_indices.insert(root_id, threads.len());
            threads.push(CommentThread {
                comments: vec![comment],
            });
        }
    }

    threads
}

/// Resolves every comment to the id of the comment that opens its thread.
///
/// Results are memoized as each chain is walked, so a deep reply chain costs one
/// step per comment rather than one walk per comment. A reply naming a parent
/// that is not in the response starts its own thread. A forge cannot produce a
/// reply cycle, but one would otherwise loop forever, so a cycle collapses to
/// its lowest id — a stable representative that keeps the comments together
/// instead of splitting or dropping them.
fn thread_roots(parents: &HashMap<u64, Option<u64>>) -> HashMap<u64, u64> {
    let mut roots: HashMap<u64, u64> = HashMap::with_capacity(parents.len());

    for &id in parents.keys() {
        if roots.contains_key(&id) {
            continue;
        }

        let mut walked: Vec<u64> = Vec::new();
        let mut current = id;
        let root = loop {
            if let Some(known) = roots.get(&current) {
                break *known;
            }
            if walked.contains(&current) {
                break walked.iter().copied().min().unwrap_or(current);
            }
            walked.push(current);
            match parents.get(&current) {
                Some(Some(parent)) if parents.contains_key(parent) => current = *parent,
                _ => break current,
            }
        };

        for node in walked {
            roots.insert(node, root);
        }
    }

    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStatus};

    const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn comment(id: u64, line: Option<u32>, in_reply_to_id: Option<u64>) -> ReviewComment {
        ReviewComment {
            id,
            author: "reviewer".into(),
            body: format!("comment {id}").into(),
            path: "src/review.rs".into(),
            side: DiffSide::Right,
            line,
            start_line: None,
            in_reply_to_id,
            is_file_level: false,
            created_at: "2026-07-25T00:00:00Z".into(),
            url: format!("https://github.com/acme/widgets/pull/1#discussion_r{id}").into(),
        }
    }

    fn reviewed_file() -> DiffFile {
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(10),
                new_line: Some(10),
                text: "context".into(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(11),
                text: "added".into(),
            },
        ];

        DiffFile {
            path: "src/review.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![DiffHunk {
                header: "@@ -10,1 +10,2 @@".into(),
                old_start: 10,
                new_start: 10,
                line_range: 0..2,
            }]
            .into(),
            counts: crate::ChangeCounts::of(&lines),
            lines: lines.into(),
        }
    }

    fn index() -> AnchorIndex {
        AnchorIndex::new(std::slice::from_ref(&reviewed_file()), HEAD.into())
    }

    #[test]
    fn anchored_comments_land_on_their_displayed_row() {
        let placed = PlacedComments::new(vec![comment(1, Some(11), None)], &index());

        let threads = placed.threads_at(0, 1);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id(), 1);
        assert!(placed.has_threads_at(0, 1));
        assert!(placed.unplaced().is_empty());
        assert_eq!(placed.thread_count(), 1);
        assert_eq!(placed.thread_count_for_file(0), 1);
    }

    #[test]
    fn replies_collapse_into_one_thread_at_the_root_position() {
        let placed = PlacedComments::new(
            vec![
                comment(1, Some(11), None),
                comment(2, Some(11), Some(1)),
                // A reply to a reply still belongs to the same thread.
                comment(3, Some(11), Some(2)),
            ],
            &index(),
        );

        let threads = placed.threads_at(0, 1);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].reply_count(), 2);
        assert_eq!(
            threads[0]
                .comments()
                .iter()
                .map(|comment| comment.id)
                .collect::<Vec<_>>(),
            [1, 2, 3],
        );
        assert_eq!(placed.thread_count(), 1);
    }

    #[test]
    fn a_replys_position_comes_from_the_thread_root() {
        // GitHub reports replies with their own line; only the root decides
        // where the thread is drawn.
        let placed = PlacedComments::new(
            vec![comment(1, Some(10), None), comment(2, Some(11), Some(1))],
            &index(),
        );

        assert_eq!(placed.threads_at(0, 0).len(), 1);
        assert!(placed.threads_at(0, 1).is_empty());
    }

    #[test]
    fn several_threads_on_one_row_keep_a_stable_order() {
        let placed = PlacedComments::new(
            vec![
                comment(30, Some(11), None),
                comment(10, Some(11), None),
                comment(20, Some(11), None),
            ],
            &index(),
        );

        assert_eq!(
            placed
                .threads_at(0, 1)
                .iter()
                .map(CommentThread::id)
                .collect::<Vec<_>>(),
            [10, 20, 30],
        );
    }

    #[test]
    fn outdated_comments_are_kept_and_marked() {
        let placed = PlacedComments::new(vec![comment(1, None, None)], &index());

        assert!(placed.threads_at(0, 0).is_empty());
        let unplaced = placed.unplaced();
        assert_eq!(unplaced.len(), 1);
        assert_eq!(unplaced[0].reason, UnplacedReason::Outdated);
        assert_eq!(unplaced[0].file, Some(0));
        assert_eq!(unplaced[0].reason.to_string(), "outdated");
        // Still counted, so the UI cannot silently lose the conversation.
        assert_eq!(placed.thread_count(), 1);
    }

    /// A whole-file comment also has no line, but calling it outdated would be
    /// wrong: nothing about it is stale.
    #[test]
    fn file_level_comments_are_not_reported_as_outdated() {
        let mut file_level = comment(1, None, None);
        file_level.is_file_level = true;
        let placed = PlacedComments::new(vec![file_level], &index());

        let unplaced = placed.unplaced();
        assert_eq!(unplaced[0].reason, UnplacedReason::FileLevel);
        assert_eq!(unplaced[0].reason.to_string(), "on the whole file");
        assert_eq!(unplaced[0].file, Some(0));
    }

    #[test]
    fn comments_outside_a_displayed_hunk_are_kept_against_their_file() {
        let mut outside = comment(1, Some(400), None);
        outside.side = DiffSide::Right;
        let placed = PlacedComments::new(vec![outside], &index());

        let unplaced = placed.unplaced();
        assert_eq!(unplaced[0].reason, UnplacedReason::NotInDiff);
        assert_eq!(unplaced[0].file, Some(0));
        assert_eq!(placed.unplaced_for_file(0).count(), 1);
    }

    #[test]
    fn comments_on_unreviewed_files_are_kept_without_a_file() {
        let mut elsewhere = comment(1, Some(10), None);
        elsewhere.path = "src/not-in-this-review.rs".into();
        let placed = PlacedComments::new(vec![elsewhere], &index());

        let unplaced = placed.unplaced();
        assert_eq!(unplaced[0].reason, UnplacedReason::PathNotReviewed);
        assert_eq!(unplaced[0].file, None);
        assert_eq!(placed.unplaced_for_file(0).count(), 0);
    }

    #[test]
    fn a_left_side_comment_resolves_against_the_base_revision() {
        let mut left = comment(1, Some(10), None);
        left.side = DiffSide::Left;
        let placed = PlacedComments::new(vec![left], &index());

        // Old line 10 is the context row, which exists on both sides.
        assert_eq!(placed.threads_at(0, 0).len(), 1);
    }

    #[test]
    fn a_reply_whose_parent_is_missing_becomes_its_own_thread() {
        // Only the reply came back; its parent is not in the response.
        let placed = PlacedComments::new(vec![comment(2, Some(11), Some(1))], &index());

        let threads = placed.threads_at(0, 1);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id(), 2);
        assert_eq!(threads[0].reply_count(), 0);
    }

    #[test]
    fn a_reply_cycle_cannot_hang_grouping() {
        let mut first = comment(1, Some(11), Some(2));
        let second = comment(2, Some(11), Some(1));
        first.in_reply_to_id = Some(2);
        let placed = PlacedComments::new(vec![first, second], &index());

        // The pair still resolves to a single bounded thread.
        assert_eq!(placed.thread_count(), 1);
    }

    #[test]
    fn multiline_comments_report_their_range() {
        let mut multiline = comment(1, Some(11), None);
        multiline.start_line = Some(10);
        assert!(multiline.is_multiline());

        let mut single = comment(2, Some(11), None);
        single.start_line = Some(11);
        assert!(!single.is_multiline());
        assert!(!comment(3, Some(11), None).is_multiline());
        assert!(!comment(4, None, None).is_multiline());
    }

    #[test]
    fn an_empty_response_places_nothing() {
        let placed = PlacedComments::new(Vec::new(), &index());
        assert!(placed.is_empty());
        assert_eq!(placed.thread_count(), 0);
        assert!(placed.threads_at(0, 0).is_empty());
    }
}
