//! Where the unsubmitted half of a review is written so it survives the process.
//!
//! Named for what it records rather than for drafts alone, because it grew past
//! them: a review in progress is draft comments, the review summary, where an
//! accepted finding came from, and which findings the reviewer has already
//! rejected. All four are state that exists only locally until a review is
//! submitted, and all four cost the reviewer real work to reconstruct.
//!
//! Declared in the domain, next to the state it records, so the storage
//! implementation depends on the domain rather than the other way round — and so a
//! view can report a write failure without knowing a database exists.

use crate::{DiffAnchor, FindingProvenance};

pub trait ReviewStateSink: Send + 'static {
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
