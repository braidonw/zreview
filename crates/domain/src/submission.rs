//! Assembling a review for submission.
//!
//! This is where the anchor validator earns its place. Every inline comment in a
//! submission has been resolved against the snapshot it claims to belong to, so a
//! position the forge would reject cannot reach the request. A draft that does not
//! resolve is reported as excluded rather than dropped or silently mangled — the
//! reviewer decides what happens to it.
//!
//! Nothing here talks to a network. Building a submission is separate from sending
//! one so that what will be posted can be shown to a human, in full, before
//! anything leaves the machine.

use std::{
    fmt::{Display, Formatter},
    sync::Arc,
};

use crate::{DiffAnchor, DiffSide, DraftComment};

/// What submitting the review asserts about it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewEvent {
    /// Remarks without a verdict.
    Comment,
    Approve,
    RequestChanges,
}

impl ReviewEvent {
    /// The value the forge's API expects.
    ///
    /// Always sent: omitting it would create a pending review on the forge, which
    /// PLAN rules out because it conflicts with an existing pending review and
    /// moves crash recovery outside this application's control.
    #[must_use]
    pub const fn github_value(self) -> &'static str {
        match self {
            Self::Comment => "COMMENT",
            Self::Approve => "APPROVE",
            Self::RequestChanges => "REQUEST_CHANGES",
        }
    }

    /// Whether the forge requires a review body for this event.
    ///
    /// GitHub requires one for `COMMENT` and `REQUEST_CHANGES`, and accepts an
    /// approval without one.
    #[must_use]
    pub const fn requires_body(self) -> bool {
        matches!(self, Self::Comment | Self::RequestChanges)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Comment => "Comment",
            Self::Approve => "Approve",
            Self::RequestChanges => "Request changes",
        }
    }
}

impl Display for ReviewEvent {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// One inline comment, at a position the snapshot confirmed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmittableComment {
    pub path: Arc<str>,
    pub side: DiffSide,
    /// 1-based line on `side`.
    pub line: u32,
    pub body: String,
}

/// A draft that will not be part of the submission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExcludedDraft {
    pub draft: DraftComment,
    pub reason: ExclusionReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExclusionReason {
    /// Its anchor no longer resolves against the current diff.
    NotAnchored,
}

impl Display for ExclusionReason {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotAnchored => "not on a line in the current diff",
        })
    }
}

/// A review, ready to be shown to a human and then sent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewSubmission {
    /// The commit the review is pinned to. Sent as `commit_id`, so the forge
    /// rejects the submission if it no longer matches the pull request's head.
    pub head_sha: Arc<str>,
    pub event: ReviewEvent,
    pub body: String,
    pub comments: Vec<SubmittableComment>,
    /// Drafts left behind, in the order they would have been posted.
    ///
    /// Never empty silently: the confirmation must show these, or a reviewer would
    /// believe they had submitted something they had not.
    pub excluded: Vec<ExcludedDraft>,
}

impl ReviewSubmission {
    /// Anchors of the drafts this submission consumes.
    ///
    /// Used to clear exactly what was posted once the forge accepts it, leaving
    /// excluded drafts untouched.
    #[must_use]
    pub fn submitted_anchors(&self) -> Vec<DiffAnchor> {
        self.comments
            .iter()
            .map(|comment| DiffAnchor {
                path: Arc::clone(&comment.path),
                side: comment.side,
                line: comment.line,
                head_sha: Arc::clone(&self.head_sha),
            })
            .collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.comments.is_empty() && self.body.trim().is_empty()
    }
}

/// Why a review cannot be assembled at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubmissionRefused {
    /// The session is not backed by a pull request.
    NotSubmittable,
    /// No comments and no summary: there is nothing to say.
    Empty,
    /// The forge requires a body for this event and there is none.
    BodyRequired(ReviewEvent),
}

impl Display for SubmissionRefused {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSubmittable => {
                formatter.write_str("this session is not a pull request, so it cannot be submitted")
            }
            Self::Empty => formatter.write_str("add a comment or a summary before submitting"),
            Self::BodyRequired(event) => write!(
                formatter,
                "a {} review needs a summary",
                event.label().to_lowercase()
            ),
        }
    }
}

impl std::error::Error for SubmissionRefused {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_map_to_the_api_and_declare_whether_a_body_is_required() {
        assert_eq!(ReviewEvent::Comment.github_value(), "COMMENT");
        assert_eq!(ReviewEvent::Approve.github_value(), "APPROVE");
        assert_eq!(
            ReviewEvent::RequestChanges.github_value(),
            "REQUEST_CHANGES",
        );

        // GitHub requires a body for these two and accepts an approval without.
        assert!(ReviewEvent::Comment.requires_body());
        assert!(ReviewEvent::RequestChanges.requires_body());
        assert!(!ReviewEvent::Approve.requires_body());
    }

    #[test]
    fn submitted_anchors_cover_only_the_posted_comments() {
        let submission = ReviewSubmission {
            head_sha: "a".repeat(40).into(),
            event: ReviewEvent::Comment,
            body: "Looks close.".to_owned(),
            comments: vec![
                SubmittableComment {
                    path: "src/a.rs".into(),
                    side: DiffSide::Right,
                    line: 4,
                    body: "one".to_owned(),
                },
                SubmittableComment {
                    path: "src/b.rs".into(),
                    side: DiffSide::Left,
                    line: 9,
                    body: "two".to_owned(),
                },
            ],
            excluded: Vec::new(),
        };

        let anchors = submission.submitted_anchors();
        assert_eq!(anchors.len(), 2);
        assert_eq!(anchors[0].path.as_ref(), "src/a.rs");
        assert_eq!(anchors[0].side, DiffSide::Right);
        assert_eq!(anchors[0].line, 4);
        // Every anchor carries the head the review is pinned to.
        assert!(
            anchors
                .iter()
                .all(|anchor| anchor.head_sha == submission.head_sha)
        );
        assert!(!submission.is_empty());
    }

    #[test]
    fn a_submission_with_neither_comments_nor_a_body_is_empty() {
        let submission = ReviewSubmission {
            head_sha: "a".repeat(40).into(),
            event: ReviewEvent::Approve,
            body: "   \n ".to_owned(),
            comments: Vec::new(),
            excluded: Vec::new(),
        };
        assert!(submission.is_empty());
    }

    #[test]
    fn refusals_explain_themselves() {
        assert!(
            SubmissionRefused::BodyRequired(ReviewEvent::RequestChanges)
                .to_string()
                .contains("request changes")
        );
        assert!(
            SubmissionRefused::Empty
                .to_string()
                .contains("before submitting")
        );
        assert!(
            SubmissionRefused::NotSubmittable
                .to_string()
                .contains("not a pull request")
        );
        assert_eq!(
            ExclusionReason::NotAnchored.to_string(),
            "not on a line in the current diff",
        );
    }
}
