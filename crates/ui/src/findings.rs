//! Showing what a review engine proposed, so a reviewer can act on it.
//!
//! PLAN section 8's last stage: findings are presented for acceptance, editing, or
//! dismissal. Nothing here decides anything. Every button hands back to
//! [`ReviewView`], which passes it to the model that owns the session, and a
//! finding only becomes a comment because the reviewer said so.
//!
//! Two things this panel is careful to show rather than hide:
//!
//! - **What was refused.** A run that rejected eleven claims and kept one looks
//!   identical to a run that found one claim, unless the eleven are visible. So the
//!   rejected count is always on screen, with its reasons on demand.
//! - **What was not looked at.** A review that skipped excluded files, or ran out of
//!   room before some of them, must not present itself as having covered the change.
//!
//! [`ReviewView`]: crate::ReviewView

use app::ReviewRunState;
use domain::{Finding, FindingId, ReviewSession, Severity};
use gpui::{AnyElement, Div, Entity, MouseButton, SharedString, div, prelude::*, px, rgb};

use crate::theme;
use crate::{ReviewView, ReviewViewEvent};

const PANEL_BACKGROUND: u32 = theme::surface::BASE;
const BORDER: u32 = theme::border::DEFAULT;
const TEXT: u32 = theme::text::PRIMARY;
const MUTED: u32 = theme::text::SECONDARY;

/// The text value of each severity, which clears 4.5:1 on the panel background.
///
/// The design pairs every severity with a dim fill and a text value; a panel row
/// is unfilled, so it takes the text value.
const fn severity_colour(severity: Severity) -> u32 {
    match severity {
        Severity::Error => theme::severity::ERROR_TEXT,
        Severity::Warning => theme::severity::WARNING_TEXT,
        Severity::Info => theme::severity::INFO_TEXT,
    }
}

/// The guidance section: what this review would be held to, and what of it will be
/// sent.
///
/// PLAN section 8 requires this before a run, and requires that the reviewer can
/// turn any of it off without editing configuration. Collapsed once a run has
/// happened, because by then the findings are what they came for. The summary
/// line stays, so what was sent is never off screen entirely.
fn render_guidance(session: &ReviewSession, expanded: bool, view: &Entity<ReviewView>) -> Div {
    let guidance = session.guidance();
    if guidance.is_empty() {
        // Discovery ran and found nothing. Saying so is not the same as showing
        // nothing: a reviewer needs to tell "this repository states no
        // conventions" from "guidance was never looked for".
        return div()
            .flex_shrink_0()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_xs()
            .text_color(rgb(MUTED))
            .child("No guidance files found. The review will judge the diff alone.");
    }
    let count = guidance.included_count();
    let kilobytes = guidance.included_bytes() / 1024;
    let excluded = guidance.excluded_paths().len();
    let toggle_view = view.clone();

    div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .id("guidance-header")
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    cx.stop_propagation();
                    toggle_view.update(cx, ReviewView::toggle_guidance_panel);
                })
                .child(div().text_color(rgb(TEXT)).child(if count == 0 {
                    SharedString::from("No guidance will be sent")
                } else {
                    SharedString::from(format!(
                        "{count} guidance file{} · {kilobytes} KB",
                        if count == 1 { "" } else { "s" }
                    ))
                }))
                .child(div().text_xs().text_color(rgb(MUTED)).child(if expanded {
                    "hide"
                } else {
                    "show"
                })),
        )
        .when(expanded, |section| {
            section
                .children(guidance.entries().iter().map(|entry| {
                    render_guidance_entry(
                        entry.path().to_string(),
                        &entry.excerpt.scope,
                        entry.bytes(),
                        entry.included,
                        view,
                    )
                }))
                // Found and not used, each with its reason. Silently dropping a
                // file the reviewer expected to matter is worse than not finding
                // it at all.
                .children(guidance.skipped().iter().map(|skip| {
                    div()
                        .px_3()
                        .py_1()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_color(rgb(theme::text::TERTIARY))
                                .child(skip.path.to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme::text::TERTIARY))
                                .child(skip.reason.to_string()),
                        )
                }))
                .when(excluded > 0, |section| {
                    section.child(
                        div()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .text_color(rgb(theme::severity::WARNING))
                            .child(format!(
                                "{excluded} file{} excluded from review by .zreview.toml",
                                if excluded == 1 { "" } else { "s" }
                            )),
                    )
                })
        })
}

fn render_guidance_entry(
    path: String,
    scope: &str,
    bytes: usize,
    included: bool,
    view: &Entity<ReviewView>,
) -> AnyElement {
    let toggle_path = path.clone();
    let view = view.clone();

    div()
        .id(SharedString::from(format!("guidance-{path}")))
        .px_3()
        .py_1()
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
            cx.stop_propagation();
            let path = toggle_path.clone();
            view.update(cx, |review, cx| review.toggle_guidance(&path, cx));
        })
        // A filled box is sent, an empty one is not. The state has to be readable
        // at a glance: this is the disclosure control.
        .child(
            div()
                .w(px(12.0))
                .h(px(12.0))
                .flex_shrink_0()
                .rounded_sm()
                .border_1()
                .border_color(rgb(if included { 0x4ade80 } else { 0x475569 }))
                .when(included, |box_| box_.bg(rgb(theme::severity::SUCCESS))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_color(rgb(if included { TEXT } else { MUTED }))
                .child(path),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(format!("{} · {}K", scope, bytes / 1024)),
        )
        .into_any_element()
}

/// The findings panel.
pub fn render(
    session: &ReviewSession,
    run: &ReviewRunState,
    selected: Option<FindingId>,
    guidance_expanded: bool,
    view: &Entity<ReviewView>,
) -> Div {
    div()
        .w(px(340.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .border_l_1()
        .border_color(rgb(BORDER))
        .bg(rgb(PANEL_BACKGROUND))
        .font_family("SF Mono")
        .text_size(px(12.0))
        .child(render_header(session, run, view))
        .child(render_guidance(session, guidance_expanded, view))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .flex()
                .flex_col()
                .children(
                    session
                        .findings()
                        .accepted()
                        .iter()
                        .map(|finding| render_finding(finding, selected == Some(finding.id), view)),
                )
                .children(render_empty_note(session, run)),
        )
        .children(render_footer(session, run))
}

fn render_header(session: &ReviewSession, run: &ReviewRunState, view: &Entity<ReviewView>) -> Div {
    let pending = session.findings().len();

    div()
        .flex_shrink_0()
        .px_3()
        .py_2()
        .flex()
        .flex_col()
        .gap_1()
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_color(rgb(TEXT))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(match pending {
                            0 => SharedString::from("Review"),
                            1 => SharedString::from("1 finding"),
                            many => SharedString::from(format!("{many} findings")),
                        }),
                )
                .child(render_run_button(run, view)),
        )
        .children(match run {
            ReviewRunState::Running { detail, .. } => Some(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(SharedString::from(detail.clone())),
            ),
            ReviewRunState::Idle
            | ReviewRunState::Complete { .. }
            | ReviewRunState::Failed { .. } => None,
        })
}

fn render_run_button(run: &ReviewRunState, view: &Entity<ReviewView>) -> AnyElement {
    let running = run.is_running();
    let (label, colour, id) = if running {
        ("Cancel", 0xb91c1c, "cancel-review")
    } else {
        ("Review", 0x2563eb, "run-review")
    };
    let view = view.clone();

    div()
        .id(SharedString::from(id))
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(colour))
        .text_xs()
        .text_color(rgb(theme::text::ON_ACCENT))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
            cx.stop_propagation();
            view.update(cx, |review, cx| {
                if running {
                    review.cancel_review(cx);
                } else {
                    cx.emit(ReviewViewEvent::ReviewRequested);
                }
            });
        })
        .child(label)
        .into_any_element()
}

fn render_finding(finding: &Finding, is_selected: bool, view: &Entity<ReviewView>) -> AnyElement {
    let id = finding.id;
    let citations: Vec<SharedString> = finding
        .guidance_sources
        .iter()
        .map(|source| SharedString::from(source.path.to_string()))
        .collect();
    // Shown as a percentage because "0.82" reads as a fraction of nothing in
    // particular, and the reviewer is deciding whether to spend attention on it.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "confidence is validated into 0..=1 before it reaches a view"
    )]
    let confidence = (finding.confidence * 100.0).round() as u32;

    div()
        .id(SharedString::from(format!("finding-{id}")))
        .px_3()
        .py_2()
        .flex()
        .flex_col()
        .gap_1()
        .border_b_1()
        .border_color(rgb(BORDER))
        .when(is_selected, |row| row.bg(rgb(theme::diff::hunk::BG)))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, {
            let view = view.clone();
            move |_, window, cx| {
                cx.stop_propagation();
                view.update(cx, |review, cx| review.reveal_finding(id, window, cx));
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_color(rgb(severity_colour(finding.severity)))
                        .child(finding.severity.label()),
                )
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .text_xs()
                        .child(format!("{confidence}%")),
                )
                .children(finding.anchor.as_ref().map(|anchor| {
                    div()
                        .text_color(rgb(MUTED))
                        .text_xs()
                        .min_w_0()
                        .overflow_hidden()
                        .child(format!("{}:{}", anchor.path, anchor.line))
                }))
                // A finding with no line is about the change as a whole, and saying
                // so is more useful than leaving the position blank.
                .children(
                    finding
                        .anchor
                        .is_none()
                        .then(|| div().text_color(rgb(MUTED)).text_xs().child("whole change")),
                ),
        )
        .child(div().text_color(rgb(TEXT)).child(finding.title.clone()))
        .when(!citations.is_empty(), |row| {
            row.child(
                div()
                    .text_xs()
                    .text_color(rgb(theme::text::TERTIARY))
                    .child(format!("per {}", citations.join(", "))),
            )
        })
        .child(
            div()
                .flex()
                .gap_2()
                .pt_1()
                .child(action_button(
                    format!("accept-{id}"),
                    "Accept",
                    0x15803d,
                    view,
                    move |review, window, cx| review.accept_finding_by_id(id, window, cx),
                ))
                .child(action_button(
                    format!("dismiss-{id}"),
                    "Dismiss",
                    0x334155,
                    view,
                    move |review, _window, cx| review.dismiss_finding_by_id(id, cx),
                )),
        )
        .into_any_element()
}

fn action_button(
    id: String,
    label: &'static str,
    colour: u32,
    view: &Entity<ReviewView>,
    action: impl Fn(&mut ReviewView, &mut gpui::Window, &mut gpui::Context<ReviewView>)
    + Clone
    + 'static,
) -> AnyElement {
    let view = view.clone();
    div()
        .id(SharedString::from(id))
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(colour))
        .text_xs()
        .text_color(rgb(theme::text::ON_ACCENT))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            let action = action.clone();
            view.update(cx, |review, cx| action(review, window, cx));
        })
        .child(label)
        .into_any_element()
}

/// What to say when there is no finding to show.
///
/// Every branch says something. An empty panel with no explanation is the failure
/// mode this whole module is trying to avoid.
fn render_empty_note(session: &ReviewSession, run: &ReviewRunState) -> Option<Div> {
    if !session.findings().is_empty() {
        return None;
    }
    let (heading, detail): (SharedString, Option<SharedString>) = match run {
        ReviewRunState::Idle => (
            "No review has been run.".into(),
            Some("Press Review to check this change against the repository's guidance.".into()),
        ),
        ReviewRunState::Running { .. } => ("Reviewing...".into(), None),
        ReviewRunState::Complete {
            rejected,
            suppressed,
            ..
        } => (
            "Nothing to act on.".into(),
            match (*rejected, *suppressed) {
                (0, 0) => Some("The review found no problems.".into()),
                (rejected, 0) => Some(
                    format!("{rejected} claim(s) did not survive checking against the diff.")
                        .into(),
                ),
                (0, suppressed) => {
                    Some(format!("{suppressed} previously dismissed claim(s) were hidden.").into())
                }
                (rejected, suppressed) => Some(
                    format!(
                        "{rejected} claim(s) did not check out and {suppressed} were previously \
                         dismissed."
                    )
                    .into(),
                ),
            },
        ),
        ReviewRunState::Failed {
            summary,
            remediation,
        } => (
            SharedString::from(summary.clone()),
            remediation.clone().map(SharedString::from),
        ),
    };

    Some(
        div()
            .px_3()
            .py_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_color(rgb(TEXT)).child(heading))
            .children(detail.map(|detail| div().text_xs().text_color(rgb(MUTED)).child(detail))),
    )
}

/// The caveats: what was refused, and what was never looked at.
fn render_footer(session: &ReviewSession, run: &ReviewRunState) -> Option<Div> {
    let rejected = session.findings().rejected().len();
    let ReviewRunState::Complete { unreviewed, .. } = run else {
        return (rejected > 0).then(|| rejected_footer(rejected));
    };

    if rejected == 0 && unreviewed.is_empty() {
        return None;
    }

    Some(
        div()
            .flex_shrink_0()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .border_t_1()
            .border_color(rgb(BORDER))
            .text_xs()
            .text_color(rgb(MUTED))
            .when(rejected > 0, |footer| {
                footer.child(format!("{rejected} claim(s) refused"))
            })
            .when(!unreviewed.is_empty(), |footer| {
                footer.child(
                    div()
                        .text_color(rgb(theme::severity::WARNING))
                        .child(format!("{} file(s) not reviewed", unreviewed.len())),
                )
            }),
    )
}

fn rejected_footer(rejected: usize) -> Div {
    div()
        .flex_shrink_0()
        .px_3()
        .py_2()
        .border_t_1()
        .border_color(rgb(BORDER))
        .text_xs()
        .text_color(rgb(MUTED))
        .child(format!("{rejected} claim(s) refused"))
}
