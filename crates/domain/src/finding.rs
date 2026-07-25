//! What a review engine proposes, and the gate everything it proposes passes
//! through.
//!
//! A backend is a program that read a diff and produced claims about it. Those
//! claims are input, not truth: a model can cite a line that is not in the diff,
//! invent a path, or return a confidence of 12. So nothing a backend returns
//! becomes a review comment directly. It arrives as a [`RawFinding`], is checked
//! against the snapshot's [`AnchorIndex`] and against field limits, and either
//! becomes a [`Finding`] carrying a resolved anchor or is reported as a
//! [`RejectedFinding`] saying why not.
//!
//! Two rules from PLAN section 8 shape this module:
//!
//! - **Only anchors the validator accepts can become inline comments.** The
//!   accepted [`Finding`] holds an [`AnchorLocation`] that was resolved during
//!   validation, so accepting one cannot fail on a position later.
//! - **A finding is a suggestion.** Nothing here writes a draft or submits
//!   anything. A human accepts, edits, or dismisses each one; that is the only way
//!   a finding becomes text that gets posted.
//!
//! Rejections are reported rather than dropped, for the same reason skipped
//! guidance and unplaced threads are: a review that silently discarded half a
//! backend's output would look identical to one that found nothing.

use std::{
    fmt::{Display, Formatter, Write as _},
    sync::Arc,
};

use crate::{AnchorError, AnchorIndex, AnchorLocation, DiffAnchor, DiffSide};

/// Longest accepted finding title.
pub const MAX_TITLE_BYTES: usize = 200;

/// Longest accepted proposed comment.
///
/// Generous enough for a comment with a code suggestion in it, small enough that
/// a backend looping on itself is caught rather than posted.
pub const MAX_COMMENT_BYTES: usize = 8 * 1024;

/// Longest accepted rationale.
pub const MAX_RATIONALE_BYTES: usize = 4 * 1024;

/// Most findings accepted from one review run.
///
/// A reviewer cannot act on more than this in one sitting, and a backend that
/// returns more has usually gone wrong rather than found more. The overflow is
/// reported rather than dropped, so a run that hit the limit says so.
pub const MAX_FINDINGS: usize = 500;

/// How much a finding claims to matter.
///
/// Ordered so that ranking is `sort` rather than a match: `Error` is greatest, so
/// a descending sort puts what matters most in front of the reviewer first.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }

    /// Parses the value a backend is asked to emit.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "info" => Some(Self::Info),
            "warning" => Some(Self::Warning),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

impl Display for Severity {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// The guidance a finding says it came from.
///
/// The hash is recorded alongside the path so a finding stays auditable after the
/// guidance file changes: a reviewer can tell whether a stored finding was made
/// against the guidance currently on disk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuidanceCitation {
    /// Repository-relative path of the guidance file.
    pub path: Arc<str>,
    /// SHA-256 of the guidance content the backend was given.
    pub content_hash: Arc<str>,
}

/// What produced a finding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FindingOrigin {
    /// A deterministic check, named by its configuration entry.
    Check(Arc<str>),
    /// A model, named by the backend that ran it.
    Ai(Arc<str>),
}

impl FindingOrigin {
    #[must_use]
    pub fn name(&self) -> &Arc<str> {
        match self {
            Self::Check(name) | Self::Ai(name) => name,
        }
    }

    /// Discriminant used in the fingerprint, so a check and a model that make the
    /// same claim about the same line stay distinguishable.
    const fn tag(&self) -> &'static str {
        match self {
            Self::Check(_) => "check",
            Self::Ai(_) => "ai",
        }
    }
}

impl Display for FindingOrigin {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Where a backend says a finding belongs, before anything has been checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawLocation {
    pub path: Arc<str>,
    pub side: DiffSide,
    /// 1-based line on `side`; the last line of a range.
    pub line: u32,
    /// First line of a range, absent for a single line.
    pub start_line: Option<u32>,
}

/// One claim from a backend, unvalidated.
///
/// Every field is exactly what the backend said, including values that cannot be
/// true. Validation happens in [`Findings::validate`], not here, so that what a
/// backend returned and what was accepted from it stay separately inspectable.
#[derive(Clone, Debug, PartialEq)]
pub struct RawFinding {
    /// Absent when the backend is making a claim about the change as a whole
    /// rather than about a line.
    pub location: Option<RawLocation>,
    pub severity: Severity,
    pub confidence: f32,
    pub title: String,
    pub rationale: String,
    /// The comment text that would be posted if a reviewer accepts this.
    pub proposed_comment: String,
    pub guidance_sources: Vec<GuidanceCitation>,
}

/// Identifies one finding within one review run.
///
/// Assigned in ranked order by [`Findings::validate`], so it is a stable handle
/// for a UI selection but says nothing about identity across runs — that is what
/// [`Finding::fingerprint`] is for.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FindingId(pub u32);

impl Display for FindingId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A finding that passed validation.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub id: FindingId,
    /// The head commit the reviewed snapshot was at.
    pub snapshot: Arc<str>,
    /// Absent for a finding about the change as a whole. When present, it resolved
    /// against the snapshot, and `location` says where it landed.
    pub anchor: Option<DiffAnchor>,
    /// Where the anchor resolved to, so accepting the finding needs no second
    /// lookup and cannot fail on position.
    pub location: Option<AnchorLocation>,
    pub severity: Severity,
    /// Between 0 and 1 inclusive.
    pub confidence: f32,
    pub title: String,
    pub rationale: String,
    pub proposed_comment: String,
    pub guidance_sources: Vec<GuidanceCitation>,
    /// Stable identity for the claim across runs. See [`fingerprint`].
    pub fingerprint: String,
    pub origin: FindingOrigin,
}

impl Finding {
    /// Whether this finding can become an inline comment.
    #[must_use]
    pub const fn is_inline(&self) -> bool {
        self.anchor.is_some()
    }
}

/// Why a finding was not accepted.
#[derive(Clone, Debug, PartialEq)]
pub enum RejectionReason {
    /// Its position does not exist in the snapshot.
    Unanchored(AnchorError),
    /// The title was empty once trimmed.
    EmptyTitle,
    /// The proposed comment was empty once trimmed, so accepting it would post
    /// nothing.
    EmptyComment,
    /// A text field was longer than its limit. Truncating would change what gets
    /// posted, so the finding is refused instead.
    TooLong {
        field: &'static str,
        bytes: usize,
        limit: usize,
    },
    /// The confidence was not a number between 0 and 1.
    ConfidenceOutOfRange(f32),
    /// An earlier finding in this run already made the same claim.
    Duplicate { fingerprint: String },
    /// The run had already accepted as many findings as it will show.
    TooMany { limit: usize },
}

impl Display for RejectionReason {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unanchored(error) => write!(formatter, "{error}"),
            Self::EmptyTitle => formatter.write_str("no title"),
            Self::EmptyComment => formatter.write_str("no comment to post"),
            Self::TooLong {
                field,
                bytes,
                limit,
            } => write!(
                formatter,
                "{field} is {bytes} bytes, over the {limit}-byte limit"
            ),
            Self::ConfidenceOutOfRange(confidence) => {
                write!(formatter, "confidence {confidence} is not between 0 and 1")
            }
            Self::Duplicate { fingerprint } => {
                write!(formatter, "already reported ({fingerprint})")
            }
            Self::TooMany { limit } => {
                write!(formatter, "over the {limit}-finding limit for one run")
            }
        }
    }
}

/// A claim that will not be shown as a finding, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct RejectedFinding {
    pub raw: RawFinding,
    pub reason: RejectionReason,
}

/// The outcome of validating one backend's output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Findings {
    accepted: Vec<Finding>,
    rejected: Vec<RejectedFinding>,
}

impl Findings {
    /// Checks raw output against a snapshot, then ranks and deduplicates what
    /// survives.
    ///
    /// Ranking runs before deduplication so that when a backend makes the same
    /// claim twice, the instance kept is the one it was most confident about.
    #[must_use]
    pub fn validate(raw: Vec<RawFinding>, anchors: &AnchorIndex, origin: &FindingOrigin) -> Self {
        let mut candidates = Vec::with_capacity(raw.len());
        let mut rejected = Vec::new();

        for finding in raw {
            match Self::check(&finding, anchors, origin) {
                Ok(checked) => candidates.push((finding, checked)),
                Err(reason) => rejected.push(RejectedFinding {
                    raw: finding,
                    reason,
                }),
            }
        }

        candidates.sort_by(|left, right| {
            Self::rank(&left.0, &left.1).cmp(&Self::rank(&right.0, &right.1))
        });

        let mut accepted: Vec<Finding> = Vec::with_capacity(candidates.len());
        for (raw, checked) in candidates {
            if accepted
                .iter()
                .any(|existing| existing.fingerprint == checked.fingerprint)
            {
                rejected.push(RejectedFinding {
                    reason: RejectionReason::Duplicate {
                        fingerprint: checked.fingerprint,
                    },
                    raw,
                });
                continue;
            }
            if accepted.len() >= MAX_FINDINGS {
                rejected.push(RejectedFinding {
                    raw,
                    reason: RejectionReason::TooMany {
                        limit: MAX_FINDINGS,
                    },
                });
                continue;
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the check above keeps the length under MAX_FINDINGS, far below u32::MAX"
            )]
            let id = FindingId(accepted.len() as u32);
            accepted.push(Finding {
                id,
                snapshot: Arc::clone(anchors.head_sha()),
                anchor: checked.anchor,
                location: checked.location,
                severity: raw.severity,
                confidence: raw.confidence,
                title: raw.title.trim().to_owned(),
                rationale: raw.rationale.trim().to_owned(),
                proposed_comment: raw.proposed_comment.trim().to_owned(),
                guidance_sources: raw.guidance_sources,
                fingerprint: checked.fingerprint,
                origin: origin.clone(),
            });
        }

        Self { accepted, rejected }
    }

    /// Sort key placing the most severe and most confident findings first, then
    /// reading down the file.
    fn rank(
        raw: &RawFinding,
        checked: &Checked,
    ) -> (std::cmp::Reverse<Severity>, u32, Arc<str>, u32) {
        let path = checked
            .anchor
            .as_ref()
            .map_or_else(|| Arc::from(""), |anchor| Arc::clone(&anchor.path));
        let line = checked.anchor.as_ref().map_or(0, |anchor| anchor.line);
        // Descending confidence as an integer, so the key stays `Ord` without
        // ordering floats.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "confidence is validated into 0..=1 before ranking"
        )]
        let confidence = 1000 - (raw.confidence * 1000.0).round() as u32;
        (std::cmp::Reverse(raw.severity), confidence, path, line)
    }

    fn check(
        raw: &RawFinding,
        anchors: &AnchorIndex,
        origin: &FindingOrigin,
    ) -> Result<Checked, RejectionReason> {
        if !(0.0..=1.0).contains(&raw.confidence) {
            return Err(RejectionReason::ConfidenceOutOfRange(raw.confidence));
        }
        if raw.title.trim().is_empty() {
            return Err(RejectionReason::EmptyTitle);
        }
        if raw.proposed_comment.trim().is_empty() {
            return Err(RejectionReason::EmptyComment);
        }
        for (field, text, limit) in [
            ("title", &raw.title, MAX_TITLE_BYTES),
            ("comment", &raw.proposed_comment, MAX_COMMENT_BYTES),
            ("rationale", &raw.rationale, MAX_RATIONALE_BYTES),
        ] {
            if text.len() > limit {
                return Err(RejectionReason::TooLong {
                    field,
                    bytes: text.len(),
                    limit,
                });
            }
        }

        let Some(location) = &raw.location else {
            return Ok(Checked {
                fingerprint: fingerprint(origin, None, &raw.title),
                anchor: None,
                location: None,
            });
        };

        let anchor = DiffAnchor {
            path: Arc::clone(&location.path),
            side: location.side,
            line: location.line,
            start_line: location.start_line,
            head_sha: Arc::clone(anchors.head_sha()),
        };
        let resolved = anchors
            .resolve(&anchor)
            .map_err(RejectionReason::Unanchored)?;

        Ok(Checked {
            fingerprint: fingerprint(origin, Some(&anchor), &raw.title),
            anchor: Some(anchor),
            location: Some(resolved),
        })
    }

    /// Findings to show, most severe first.
    #[must_use]
    pub fn accepted(&self) -> &[Finding] {
        &self.accepted
    }

    /// Claims that were refused, in the order they were refused.
    #[must_use]
    pub fn rejected(&self) -> &[RejectedFinding] {
        &self.rejected
    }

    /// Removes a finding by id, returning it.
    ///
    /// Used when a reviewer accepts or dismisses one: a finding that has been acted
    /// on is no longer pending.
    pub fn take(&mut self, id: FindingId) -> Option<Finding> {
        let index = self.accepted.iter().position(|finding| finding.id == id)?;
        Some(self.accepted.remove(index))
    }

    #[must_use]
    pub fn get(&self, id: FindingId) -> Option<&Finding> {
        self.accepted.iter().find(|finding| finding.id == id)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.accepted.len()
    }
}

/// What validation worked out about a candidate.
#[derive(Clone, Debug)]
struct Checked {
    fingerprint: String,
    anchor: Option<DiffAnchor>,
    location: Option<AnchorLocation>,
}

/// Stable identity for a claim, for deduplication across runs.
///
/// Built from what makes two findings the same claim — who found it, where, and
/// what it says — and deliberately not from the proposed comment, whose wording
/// varies between runs that found the identical problem.
///
/// This is FNV-1a rather than a cryptographic hash: a fingerprint decides whether
/// two findings are the same, and nothing trusts it for anything else. Keeping it
/// arithmetic keeps the functional core free of dependencies.
#[must_use]
pub fn fingerprint(origin: &FindingOrigin, anchor: Option<&DiffAnchor>, title: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    let mut absorb = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        // A separator, so that ("ab", "c") and ("a", "bc") differ.
        hash ^= 0xff;
        hash = hash.wrapping_mul(PRIME);
    };

    absorb(origin.tag().as_bytes());
    absorb(origin.name().as_bytes());
    match anchor {
        Some(anchor) => {
            absorb(anchor.path.as_bytes());
            absorb(anchor.side.github_value().as_bytes());
            absorb(anchor.line.to_string().as_bytes());
            absorb(
                anchor
                    .start_line
                    .unwrap_or(anchor.line)
                    .to_string()
                    .as_bytes(),
            );
        }
        None => absorb(b"whole-change"),
    }
    absorb(normalized_title(title).as_bytes());

    let mut hex = String::with_capacity(16);
    write!(hex, "{hash:016x}").expect("writing to a String cannot fail");
    hex
}

/// Lowercased with runs of whitespace collapsed, so that reflowing a title does
/// not make it a different claim.
fn normalized_title(title: &str) -> String {
    title
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChangeCounts, DiffFile, DiffHunk, DiffLine, DiffLineKind, FileStatus};

    const HEAD: &str = "head0000";

    fn file(path: &str) -> DiffFile {
        // Two hunks, so a range that crosses them can be tested.
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(1),
                new_line: Some(1),
                text: Arc::from("one"),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(2),
                text: Arc::from("two"),
            },
            DiffLine {
                kind: DiffLineKind::Deletion,
                old_line: Some(2),
                new_line: None,
                text: Arc::from("gone"),
            },
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(20),
                new_line: Some(20),
                text: Arc::from("far"),
            },
        ];
        DiffFile {
            path: Arc::from(path),
            old_path: None,
            status: FileStatus::Modified,
            is_binary: false,
            hunks: Arc::from(vec![
                DiffHunk {
                    header: Arc::from("@@ -1,2 +1,2 @@"),
                    old_start: 1,
                    new_start: 1,
                    line_range: 0..3,
                },
                DiffHunk {
                    header: Arc::from("@@ -20,1 +20,1 @@"),
                    old_start: 20,
                    new_start: 20,
                    line_range: 3..4,
                },
            ]),
            counts: ChangeCounts::of(&lines),
            lines: Arc::from(lines),
        }
    }

    fn index() -> AnchorIndex {
        AnchorIndex::new(&[file("src/main.rs")], Arc::from(HEAD))
    }

    fn origin() -> FindingOrigin {
        FindingOrigin::Ai(Arc::from("claude-code"))
    }

    fn raw(title: &str, line: Option<u32>) -> RawFinding {
        RawFinding {
            location: line.map(|line| RawLocation {
                path: Arc::from("src/main.rs"),
                side: DiffSide::Right,
                line,
                start_line: None,
            }),
            severity: Severity::Warning,
            confidence: 0.8,
            title: title.to_owned(),
            rationale: "because".to_owned(),
            proposed_comment: "please fix".to_owned(),
            guidance_sources: Vec::new(),
        }
    }

    #[test]
    fn accepts_a_finding_on_a_line_in_the_diff() {
        let findings =
            Findings::validate(vec![raw("no error handling", Some(2))], &index(), &origin());

        assert_eq!(findings.accepted().len(), 1);
        assert!(findings.rejected().is_empty());
        let finding = &findings.accepted()[0];
        assert!(finding.is_inline());
        assert_eq!(finding.location, Some(AnchorLocation { file: 0, row: 1 }));
        assert_eq!(&*finding.snapshot, HEAD);
    }

    #[test]
    fn rejects_a_line_that_is_not_in_the_diff() {
        let findings = Findings::validate(vec![raw("invented", Some(9))], &index(), &origin());

        assert!(findings.accepted().is_empty());
        assert!(matches!(
            findings.rejected()[0].reason,
            RejectionReason::Unanchored(AnchorError::LineNotInDiff { line: 9, .. })
        ));
    }

    #[test]
    fn rejects_a_path_that_is_not_under_review() {
        let mut finding = raw("elsewhere", Some(2));
        finding.location.as_mut().unwrap().path = Arc::from("src/other.rs");

        let findings = Findings::validate(vec![finding], &index(), &origin());

        assert!(matches!(
            findings.rejected()[0].reason,
            RejectionReason::Unanchored(AnchorError::UnknownPath(_))
        ));
    }

    #[test]
    fn rejects_a_range_that_crosses_hunks() {
        let mut finding = raw("spans a gap", Some(20));
        finding.location.as_mut().unwrap().start_line = Some(1);

        let findings = Findings::validate(vec![finding], &index(), &origin());

        assert!(matches!(
            findings.rejected()[0].reason,
            RejectionReason::Unanchored(AnchorError::RangeCrossesHunks { .. })
        ));
    }

    #[test]
    fn keeps_a_finding_about_the_whole_change_without_an_anchor() {
        let findings =
            Findings::validate(vec![raw("no tests anywhere", None)], &index(), &origin());

        let finding = &findings.accepted()[0];
        assert!(!finding.is_inline());
        assert_eq!(finding.anchor, None);
        assert_eq!(finding.location, None);
    }

    #[test]
    fn rejects_an_impossible_confidence() {
        let mut finding = raw("overconfident", Some(2));
        finding.confidence = 12.0;

        let findings = Findings::validate(vec![finding], &index(), &origin());

        assert!(matches!(
            findings.rejected()[0].reason,
            RejectionReason::ConfidenceOutOfRange(_)
        ));
    }

    #[test]
    fn rejects_a_not_a_number_confidence() {
        let mut finding = raw("nan", Some(2));
        finding.confidence = f32::NAN;

        let findings = Findings::validate(vec![finding], &index(), &origin());

        assert!(matches!(
            findings.rejected()[0].reason,
            RejectionReason::ConfidenceOutOfRange(_)
        ));
    }

    #[test]
    fn rejects_a_finding_with_nothing_to_post() {
        let mut finding = raw("titled but silent", Some(2));
        finding.proposed_comment = "   \n ".to_owned();

        let findings = Findings::validate(vec![finding], &index(), &origin());

        assert_eq!(findings.rejected()[0].reason, RejectionReason::EmptyComment);
    }

    #[test]
    fn refuses_an_oversized_comment_rather_than_truncating_it() {
        let mut finding = raw("runaway", Some(2));
        finding.proposed_comment = "x".repeat(MAX_COMMENT_BYTES + 1);

        let findings = Findings::validate(vec![finding], &index(), &origin());

        assert!(matches!(
            findings.rejected()[0].reason,
            RejectionReason::TooLong {
                field: "comment",
                ..
            }
        ));
    }

    #[test]
    fn ranks_more_severe_findings_first() {
        let mut warning = raw("a warning", Some(1));
        let mut error = raw("an error", Some(2));
        error.severity = Severity::Error;
        let mut info = raw("an info", Some(20));
        info.severity = Severity::Info;

        warning.severity = Severity::Warning;
        let findings = Findings::validate(vec![info, warning, error], &index(), &origin());

        let severities: Vec<_> = findings
            .accepted()
            .iter()
            .map(|finding| finding.severity)
            .collect();
        assert_eq!(
            severities,
            vec![Severity::Error, Severity::Warning, Severity::Info]
        );
    }

    #[test]
    fn ranks_the_more_confident_of_two_equally_severe_findings_first() {
        let mut unsure = raw("maybe", Some(1));
        unsure.confidence = 0.2;
        let mut sure = raw("definitely", Some(2));
        sure.confidence = 0.95;

        let findings = Findings::validate(vec![unsure, sure], &index(), &origin());

        assert_eq!(findings.accepted()[0].title, "definitely");
    }

    #[test]
    fn collapses_the_same_claim_reported_twice_and_keeps_the_confident_one() {
        let mut weak = raw("Missing  error handling", Some(2));
        weak.confidence = 0.3;
        let mut strong = raw("missing error handling", Some(2));
        strong.confidence = 0.9;

        let findings = Findings::validate(vec![weak, strong], &index(), &origin());

        assert_eq!(findings.accepted().len(), 1);
        assert!((findings.accepted()[0].confidence - 0.9).abs() < f32::EPSILON);
        assert!(matches!(
            findings.rejected()[0].reason,
            RejectionReason::Duplicate { .. }
        ));
    }

    #[test]
    fn the_same_claim_on_different_lines_is_two_findings() {
        let findings = Findings::validate(
            vec![
                raw("missing error handling", Some(1)),
                raw("missing error handling", Some(2)),
            ],
            &index(),
            &origin(),
        );

        assert_eq!(findings.accepted().len(), 2);
    }

    #[test]
    fn a_check_and_a_model_making_the_same_claim_do_not_collide() {
        let anchor = DiffAnchor::single(
            Arc::from("src/main.rs"),
            DiffSide::Right,
            2,
            Arc::from(HEAD),
        );
        let by_check = fingerprint(
            &FindingOrigin::Check(Arc::from("clippy")),
            Some(&anchor),
            "unused",
        );
        let by_model = fingerprint(
            &FindingOrigin::Ai(Arc::from("clippy")),
            Some(&anchor),
            "unused",
        );

        assert_ne!(by_check, by_model);
    }

    #[test]
    fn fingerprints_ignore_title_whitespace_and_case() {
        let anchor = DiffAnchor::single(
            Arc::from("src/main.rs"),
            DiffSide::Right,
            2,
            Arc::from(HEAD),
        );
        let origin = origin();

        assert_eq!(
            fingerprint(&origin, Some(&anchor), "Missing   error\nhandling"),
            fingerprint(&origin, Some(&anchor), "missing error handling")
        );
    }

    #[test]
    fn fingerprint_field_boundaries_cannot_be_shifted() {
        let origin = FindingOrigin::Ai(Arc::from("ab"));
        let shifted = FindingOrigin::Ai(Arc::from("abc"));

        assert_ne!(
            fingerprint(&origin, None, "cx"),
            fingerprint(&shifted, None, "x")
        );
    }

    #[test]
    fn a_runaway_backend_is_capped_and_says_so() {
        // Distinct titles so nothing is collapsed as a duplicate first.
        let raw: Vec<_> = (0..MAX_FINDINGS + 3)
            .map(|index| raw(&format!("finding {index}"), Some(2)))
            .collect();

        let findings = Findings::validate(raw, &index(), &origin());

        assert_eq!(findings.len(), MAX_FINDINGS);
        assert_eq!(findings.rejected().len(), 3);
        assert!(matches!(
            findings.rejected()[0].reason,
            RejectionReason::TooMany {
                limit: MAX_FINDINGS
            }
        ));
    }

    #[test]
    fn taking_a_finding_removes_it() {
        let mut findings = Findings::validate(
            vec![raw("first", Some(1)), raw("second", Some(2))],
            &index(),
            &origin(),
        );
        let id = findings.accepted()[0].id;

        let taken = findings.take(id).expect("the finding is pending");

        assert_eq!(taken.id, id);
        assert_eq!(findings.len(), 1);
        assert!(findings.take(id).is_none());
    }
}
