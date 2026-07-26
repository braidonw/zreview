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

use crate::{DiffAnchor, DiffSide, FindingProvenance};

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

    /// Records the review summary for a snapshot.
    ///
    /// Keyed by head rather than by anchor, because the summary belongs to the
    /// review as a whole rather than to any line.
    fn save_summary(&self, head_sha: &str, body: &str);

    /// Records that every draft at these anchors has been submitted and is no
    /// longer local, along with the summary that went with them.
    ///
    /// Called only after a forge has accepted the review: until then the local
    /// copy is the only copy.
    fn clear_submitted(&self, head_sha: &str, anchors: &[DiffAnchor]);

    /// Records that the draft at an anchor began as a backend's finding.
    ///
    /// Sent after [`save`], so the draft's text is durable before the note about
    /// where it came from. Losing provenance costs an attribution; losing the text
    /// costs the reviewer their words.
    ///
    /// [`save`]: Self::save
    fn save_provenance(&self, anchor: &DiffAnchor, provenance: &FindingProvenance);

    /// Records that the reviewer rejected a claim against this snapshot.
    ///
    /// Keyed by head, because a dismissal is a judgement about code at a
    /// particular revision: a force-push that rewrites the line should let the
    /// finding be raised again rather than keeping it silently suppressed.
    fn dismiss_finding(&self, head_sha: &str, fingerprint: &str);

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
#[derive(Clone, Debug, PartialEq)]
pub struct DraftComment {
    pub anchor: DiffAnchor,
    pub body: String,
    /// Set when the anchor no longer resolves against the current diff.
    ///
    /// A stale draft still holds its text and is still shown, but it cannot be
    /// submitted inline until it is re-anchored.
    pub is_stale: bool,
    /// Set when this draft started life as a backend's finding.
    ///
    /// The reviewer accepted it, so it is their comment now and posts under their
    /// name — but which model proposed it and which guidance it rested on stay
    /// recorded, both because PLAN section 8 asks findings to be auditable and
    /// because a reviewer scanning the queue should be able to tell their own words
    /// from a suggestion they took.
    pub provenance: Option<FindingProvenance>,
}

impl DraftComment {
    /// Whether a backend proposed this rather than the reviewer writing it.
    #[must_use]
    pub const fn is_proposed(&self) -> bool {
        self.provenance.is_some()
    }
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
        self.insert_with(anchor, body, None, file, row);
    }

    /// Stores a draft, recording where it came from when a backend proposed it.
    ///
    /// Editing a proposed draft keeps its provenance: the reviewer reworded a
    /// suggestion, which does not make it stop having been one.
    pub fn insert_with(
        &mut self,
        anchor: DiffAnchor,
        body: String,
        provenance: Option<FindingProvenance>,
        file: usize,
        row: usize,
    ) {
        let key = DraftKey::of(&anchor);
        self.by_row.insert((file, row), key.clone());
        // An edit that arrives without provenance must not erase provenance already
        // recorded here, or accepting a finding and then rewording it would lose the
        // attribution.
        let provenance = provenance.or_else(|| {
            self.entries
                .get(&key)
                .and_then(|existing| existing.provenance.clone())
        });
        self.entries.insert(
            key,
            DraftComment {
                anchor,
                body,
                is_stale: false,
                provenance,
            },
        );
    }

    /// Stores a draft whose anchor no longer resolves to a displayed row.
    pub fn insert_stale(&mut self, anchor: DiffAnchor, body: String) {
        let key = DraftKey::of(&anchor);
        let provenance = self
            .entries
            .get(&key)
            .and_then(|existing| existing.provenance.clone());
        self.entries.insert(
            key,
            DraftComment {
                anchor,
                body,
                is_stale: true,
                provenance,
            },
        );
    }

    /// Removes the draft at a row, returning it.
    pub fn remove_at(&mut self, file: usize, row: usize) -> Option<DraftComment> {
        let key = self.by_row.remove(&(file, row))?;
        self.entries.remove(&key)
    }

    /// Removes an anchored draft by its anchor, keeping the row index consistent.
    ///
    /// Used after submission, when what to forget is known by position rather than
    /// by row.
    pub fn remove_anchored(&mut self, anchor: &DiffAnchor) -> Option<DraftComment> {
        let key = DraftKey::of(anchor);
        let removed = self.entries.remove(&key)?;
        self.by_row.retain(|_, row_key| *row_key != key);
        Some(removed)
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

    /// Attaches provenance to a draft that is already here.
    ///
    /// Returns `false` when no draft holds that anchor, which is how restoring
    /// skips provenance for a draft that has since been submitted or discarded.
    pub fn set_provenance(&mut self, anchor: &DiffAnchor, provenance: FindingProvenance) -> bool {
        let Some(draft) = self.entries.get_mut(&DraftKey::of(anchor)) else {
            return false;
        };
        draft.provenance = Some(provenance);
        true
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
            start_line: None,
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
