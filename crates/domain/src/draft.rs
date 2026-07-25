//! Unsubmitted review comments the reviewer is writing.
//!
//! A draft is text plus the anchor it will be submitted against. The anchor is
//! validated when the draft is created, so a draft that exists is a draft that
//! GitHub would accept — with one exception this module is careful about: a draft
//! restored from an earlier session may no longer resolve, because the diff it was
//! written against can have changed underneath it. Such a draft is marked stale
//! and kept. Losing a reviewer's words is the one outcome that is never
//! acceptable, so nothing here deletes text the reviewer did not delete.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use crate::{DiffAnchor, DiffSide};

/// Where draft changes are written so they survive the process.
///
/// Declared here, next to the drafts themselves, so the storage implementation
/// depends on the domain rather than the other way round — and so a view can
/// report a write failure without knowing a database exists.
///
/// Implementations must not block: this is called from the UI thread as the
/// reviewer types. `Send` because a sink is built wherever loading happens, which
/// is not the thread that will use it.
pub trait DraftSink: Send + 'static {
    /// Records the current text at an anchor.
    fn save(&self, anchor: &DiffAnchor, body: &str);

    /// Records that the draft at an anchor is gone.
    fn discard(&self, anchor: &DiffAnchor);

    /// The most recent write failure, if writes are failing.
    fn failure(&self) -> Option<String>;
}

/// A draft's position, ordered so a queue reads down the file.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DraftKey {
    path: Arc<str>,
    line: u32,
    side: DiffSide,
}

impl DraftKey {
    fn of(anchor: &DiffAnchor) -> Self {
        Self {
            path: Arc::clone(&anchor.path),
            line: anchor.line,
            side: anchor.side,
        }
    }
}

/// One unsubmitted comment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraftComment {
    pub anchor: DiffAnchor,
    pub body: String,
    /// Set when the anchor no longer resolves against the current diff.
    ///
    /// A stale draft still holds its text and is still shown, but it cannot be
    /// submitted inline until it is re-anchored.
    pub is_stale: bool,
}

/// Every draft in one session.
///
/// Iteration order is by path then line, which is the order a reviewer reads and
/// the order a submission should list them in.
#[derive(Clone, Debug, Default)]
pub struct Drafts {
    entries: BTreeMap<DraftKey, DraftComment>,
    /// Row lookup for the virtualized diff, rebuilt as drafts change.
    by_row: HashMap<(usize, usize), DraftKey>,
}

impl Drafts {
    /// Stores a draft that resolved to a displayed row.
    ///
    /// Replaces any draft already at that anchor: one anchor holds one draft, so
    /// reopening the composer on a line edits what is there rather than stacking
    /// a second comment on the same line.
    pub fn insert(&mut self, anchor: DiffAnchor, body: String, file: usize, row: usize) {
        let key = DraftKey::of(&anchor);
        self.by_row.insert((file, row), key.clone());
        self.entries.insert(
            key,
            DraftComment {
                anchor,
                body,
                is_stale: false,
            },
        );
    }

    /// Stores a draft whose anchor no longer resolves to a displayed row.
    pub fn insert_stale(&mut self, anchor: DiffAnchor, body: String) {
        let key = DraftKey::of(&anchor);
        self.entries.insert(
            key,
            DraftComment {
                anchor,
                body,
                is_stale: true,
            },
        );
    }

    /// Removes the draft at a row, returning it.
    pub fn remove_at(&mut self, file: usize, row: usize) -> Option<DraftComment> {
        let key = self.by_row.remove(&(file, row))?;
        self.entries.remove(&key)
    }

    /// Removes a stale draft by its old anchor, returning its text.
    ///
    /// Only stale drafts can be taken this way. An anchored draft is reachable by
    /// row, and letting it be pulled out by anchor would leave the row index
    /// pointing at nothing.
    pub fn take_stale(&mut self, anchor: &DiffAnchor) -> Option<String> {
        let key = DraftKey::of(anchor);
        if !self.entries.get(&key).is_some_and(|draft| draft.is_stale) {
            return None;
        }
        self.entries.remove(&key).map(|draft| draft.body)
    }

    #[must_use]
    pub fn at(&self, file: usize, row: usize) -> Option<&DraftComment> {
        let key = self.by_row.get(&(file, row))?;
        self.entries.get(key)
    }

    #[must_use]
    pub fn get(&self, anchor: &DiffAnchor) -> Option<&DraftComment> {
        self.entries.get(&DraftKey::of(anchor))
    }

    /// Every draft, in queue order.
    pub fn iter(&self) -> impl Iterator<Item = &DraftComment> {
        self.entries.values()
    }

    /// Drafts that cannot currently be submitted inline.
    pub fn stale(&self) -> impl Iterator<Item = &DraftComment> {
        self.entries.values().filter(|draft| draft.is_stale)
    }

    /// Drafts on one file, whether anchored or stale.
    pub fn for_path(&self, path: &str) -> impl Iterator<Item = &DraftComment> {
        self.entries
            .values()
            .filter(move |draft| draft.anchor.path.as_ref() == path)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn stale_count(&self) -> usize {
        self.stale().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(path: &str, line: u32, side: DiffSide) -> DiffAnchor {
        DiffAnchor {
            path: path.into(),
            side,
            line,
            head_sha: "a".repeat(40).into(),
        }
    }

    #[test]
    fn a_draft_is_reachable_by_row_and_by_anchor() {
        let mut drafts = Drafts::default();
        let anchor = anchor("src/review.rs", 11, DiffSide::Right);
        drafts.insert(anchor.clone(), "needs a test".to_owned(), 0, 6);

        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts.at(0, 6).unwrap().body, "needs a test");
        assert_eq!(drafts.get(&anchor).unwrap().body, "needs a test");
        assert!(!drafts.at(0, 6).unwrap().is_stale);
        assert!(drafts.at(0, 7).is_none());
    }

    #[test]
    fn one_anchor_holds_one_draft() {
        let mut drafts = Drafts::default();
        let anchor = anchor("src/review.rs", 11, DiffSide::Right);
        drafts.insert(anchor.clone(), "first thought".to_owned(), 0, 6);
        drafts.insert(anchor, "second thought".to_owned(), 0, 6);

        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts.at(0, 6).unwrap().body, "second thought");
    }

    /// The same line number on opposite sides is two different positions.
    #[test]
    fn the_two_sides_of_a_line_hold_separate_drafts() {
        let mut drafts = Drafts::default();
        drafts.insert(
            anchor("src/review.rs", 10, DiffSide::Right),
            "on the new line".to_owned(),
            0,
            3,
        );
        drafts.insert(
            anchor("src/review.rs", 10, DiffSide::Left),
            "on the old line".to_owned(),
            0,
            3,
        );

        assert_eq!(drafts.len(), 2);
    }

    #[test]
    fn removing_a_draft_frees_its_row() {
        let mut drafts = Drafts::default();
        drafts.insert(
            anchor("src/review.rs", 11, DiffSide::Right),
            "needs a test".to_owned(),
            0,
            6,
        );

        let removed = drafts.remove_at(0, 6).unwrap();
        assert_eq!(removed.body, "needs a test");
        assert!(drafts.is_empty());
        assert!(drafts.at(0, 6).is_none());
        assert!(drafts.remove_at(0, 6).is_none());
    }

    #[test]
    fn the_queue_reads_down_the_files() {
        let mut drafts = Drafts::default();
        drafts.insert(
            anchor("src/second.rs", 5, DiffSide::Right),
            "c".to_owned(),
            1,
            0,
        );
        drafts.insert(
            anchor("src/first.rs", 40, DiffSide::Right),
            "b".to_owned(),
            0,
            9,
        );
        drafts.insert(
            anchor("src/first.rs", 4, DiffSide::Right),
            "a".to_owned(),
            0,
            1,
        );

        assert_eq!(
            drafts
                .iter()
                .map(|draft| draft.body.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"],
        );
    }

    #[test]
    fn a_stale_draft_keeps_its_text_but_has_no_row() {
        let mut drafts = Drafts::default();
        let stale = anchor("src/review.rs", 400, DiffSide::Right);
        drafts.insert_stale(stale.clone(), "written against an older diff".to_owned());

        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts.stale_count(), 1);
        assert_eq!(
            drafts.get(&stale).unwrap().body,
            "written against an older diff",
        );
        // Nothing to attach it to in the current diff.
        assert!(drafts.at(0, 0).is_none());
    }

    #[test]
    fn drafts_can_be_listed_per_file() {
        let mut drafts = Drafts::default();
        drafts.insert(
            anchor("src/first.rs", 4, DiffSide::Right),
            "a".to_owned(),
            0,
            1,
        );
        drafts.insert(
            anchor("src/second.rs", 5, DiffSide::Right),
            "b".to_owned(),
            1,
            0,
        );
        drafts.insert_stale(anchor("src/first.rs", 900, DiffSide::Right), "c".to_owned());

        let first = drafts
            .for_path("src/first.rs")
            .map(|draft| draft.body.as_str())
            .collect::<Vec<_>>();
        assert_eq!(first, ["a", "c"], "stale drafts still belong to their file");
        assert_eq!(drafts.for_path("src/second.rs").count(), 1);
        assert_eq!(drafts.for_path("src/absent.rs").count(), 0);
    }
}
