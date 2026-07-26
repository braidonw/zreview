//! What a review will be held to, and what the reviewer has decided about it.
//!
//! PLAN section 8 requires that before a review runs, the reviewer sees every
//! guidance file that was discovered, what it applies to, and whether it will be
//! sent — and can turn any of it off. This is the state behind that panel.
//!
//! It is deliberately the *input* to a run rather than a preview of one. A panel
//! that showed what discovery found while the run went off and re-discovered its
//! own copy would be a disclosure notice that could quietly disagree with reality.
//! So a run reads what is here, and what the reviewer sees is what is sent.
//!
//! Discovery itself lives in the review crate, which reads files and compiles
//! globs. This type holds only the results, so the functional core keeps no
//! filesystem knowledge and a view can render the panel from the domain alone.

use std::sync::Arc;

use crate::GuidanceExcerpt;

/// One guidance file discovery found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuidanceEntry {
    pub excerpt: GuidanceExcerpt,
    /// Whether it will be sent. Discovery includes everything it finds; the
    /// reviewer can turn any of it off.
    pub included: bool,
}

impl GuidanceEntry {
    #[must_use]
    pub fn path(&self) -> &Arc<str> {
        &self.excerpt.path
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        self.excerpt.content.len()
    }
}

/// Something discovery found but will not use, and why.
///
/// The reason arrives already rendered, so the domain does not need to know the
/// shape of a size limit or a glob error to display one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuidanceSkip {
    pub path: Arc<str>,
    pub reason: Arc<str>,
}

/// Everything discovery found for one snapshot, and what will be sent.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GuidanceSelection {
    entries: Vec<GuidanceEntry>,
    skipped: Vec<GuidanceSkip>,
    /// Files in this snapshot that configuration excludes from review.
    ///
    /// Resolved when discovery ran, against the paths actually under review, so
    /// nothing here needs to match a glob. That keeps the pattern matching in the
    /// crate that owns the config format and leaves this a plain list.
    excluded_paths: Vec<Arc<str>>,
}

impl GuidanceSelection {
    #[must_use]
    pub fn new(
        entries: Vec<GuidanceEntry>,
        skipped: Vec<GuidanceSkip>,
        excluded_paths: Vec<Arc<str>>,
    ) -> Self {
        Self {
            entries,
            skipped,
            excluded_paths,
        }
    }

    /// Reviewed files configuration keeps out of the review.
    #[must_use]
    pub fn excluded_paths(&self) -> &[Arc<str>] {
        &self.excluded_paths
    }

    #[must_use]
    pub fn excludes(&self, path: &str) -> bool {
        self.excluded_paths
            .iter()
            .any(|excluded| &**excluded == path)
    }

    #[must_use]
    pub fn entries(&self) -> &[GuidanceEntry] {
        &self.entries
    }

    /// Candidates that were found and will not be used.
    #[must_use]
    pub fn skipped(&self) -> &[GuidanceSkip] {
        &self.skipped
    }

    /// What a run will actually send.
    pub fn included(&self) -> impl Iterator<Item = &GuidanceExcerpt> {
        self.entries
            .iter()
            .filter(|entry| entry.included)
            .map(|entry| &entry.excerpt)
    }

    #[must_use]
    pub fn included_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.included).count()
    }

    /// How much guidance a run would send, for a reviewer deciding whether that is
    /// more of their code's context than they want leaving the machine.
    #[must_use]
    pub fn included_bytes(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.included)
            .map(GuidanceEntry::bytes)
            .sum()
    }

    /// Turns one file on or off. Returns whether anything changed.
    pub fn set_included(&mut self, path: &str, included: bool) -> bool {
        self.entries
            .iter_mut()
            .find(|entry| &*entry.excerpt.path == path)
            .is_some_and(|entry| {
                let changed = entry.included != included;
                entry.included = included;
                changed
            })
    }

    /// Flips one file. Returns its new state.
    pub fn toggle(&mut self, path: &str) -> Option<bool> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| &*entry.excerpt.path == path)?;
        entry.included = !entry.included;
        Some(entry.included)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.skipped.is_empty() && self.excluded_paths.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, content: &str, included: bool) -> GuidanceEntry {
        GuidanceEntry {
            excerpt: GuidanceExcerpt {
                path: path.into(),
                scope: "whole repository".into(),
                content: content.to_owned(),
                content_hash: "hash".into(),
            },
            included,
        }
    }

    #[test]
    fn only_included_guidance_would_be_sent() {
        let selection = GuidanceSelection::new(
            vec![
                entry("AGENTS.md", "aaaa", true),
                entry("CLAUDE.md", "bb", false),
            ],
            Vec::new(),
            Vec::new(),
        );

        let sent: Vec<_> = selection
            .included()
            .map(|excerpt| excerpt.path.to_string())
            .collect();
        assert_eq!(sent, vec!["AGENTS.md".to_owned()]);
        assert_eq!(selection.included_count(), 1);
        assert_eq!(selection.included_bytes(), 4);
    }

    #[test]
    fn turning_a_file_off_removes_it_from_what_would_be_sent() {
        let mut selection = GuidanceSelection::new(
            vec![entry("AGENTS.md", "aaaa", true)],
            Vec::new(),
            Vec::new(),
        );

        assert!(selection.set_included("AGENTS.md", false));
        assert_eq!(selection.included_count(), 0);
        assert_eq!(selection.included_bytes(), 0);
        // Setting it to what it already is changes nothing.
        assert!(!selection.set_included("AGENTS.md", false));
    }

    #[test]
    fn toggling_reports_the_new_state() {
        let mut selection =
            GuidanceSelection::new(vec![entry("AGENTS.md", "a", true)], Vec::new(), Vec::new());

        assert_eq!(selection.toggle("AGENTS.md"), Some(false));
        assert_eq!(selection.toggle("AGENTS.md"), Some(true));
        assert_eq!(selection.toggle("NOT-FOUND.md"), None);
    }

    #[test]
    fn a_skipped_candidate_keeps_the_reason_it_was_skipped() {
        let selection = GuidanceSelection::new(
            Vec::new(),
            vec![GuidanceSkip {
                path: "HUGE.md".into(),
                reason: "90000 bytes, over the 65536-byte limit".into(),
            }],
            Vec::new(),
        );

        assert!(!selection.is_empty(), "a skip is still worth showing");
        assert_eq!(selection.skipped()[0].path.as_ref(), "HUGE.md");
    }
}
