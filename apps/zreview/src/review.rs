//! Driving a review run without blocking the UI thread.
//!
//! The same shape as [`crate::loading`], and for the same reason: a review is
//! subprocess work that takes minutes, so it runs on the background executor while
//! a foreground task publishes what it reports. The difference is that a review can
//! be abandoned, so the cancellation flag is created here and handed to both sides —
//! the view sets it, the backend polls it.

use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use domain::{Findings, ReviewError, ReviewEventSink, ReviewProgress, ReviewSession};
use gpui::{App, Entity, WindowHandle};
use review::{Agent, CodingAgent};
use ui::SessionView;

/// How often the foreground task publishes progress.
///
/// Matches the loader's cadence. A review reports far less often than that, so this
/// is about the delay before a line appears, not about how much is displayed.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// What the background review hands to the foreground task.
#[derive(Default)]
struct Handoff {
    detail: Option<String>,
    result: Option<Result<Outcome, ReviewError>>,
}

/// A finished run, in the shape the view needs.
struct Outcome {
    findings: Findings,
    /// Files the review did not see, so a partial review cannot present itself as
    /// a complete one.
    unreviewed: Vec<String>,
}

/// Publishes progress into the handoff and reports cancellation back out.
struct Channel {
    handoff: Arc<Mutex<Handoff>>,
    cancel: Arc<AtomicBool>,
}

impl ReviewEventSink for Channel {
    fn progress(&self, progress: ReviewProgress) {
        lock(&self.handoff).detail = Some(progress.to_string());
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Runs a review of `session` in the background and publishes it into `window`.
///
/// The session is cloned rather than borrowed: the background thread must not hold
/// anything the UI thread is also mutating, and a snapshot's files are behind `Arc`
/// so the copy is cheap.
pub fn spawn(
    window: WindowHandle<SessionView>,
    review_view: Entity<ui::ReviewView>,
    session: ReviewSession,
    cx: &mut App,
) {
    let handoff = Arc::new(Mutex::new(Handoff::default()));
    let cancel = Arc::new(AtomicBool::new(false));

    // Told before the work starts, so the panel shows a running review immediately
    // rather than after the first progress report.
    review_view.update(cx, |review, cx| {
        review.review_started(Arc::clone(&cancel), cx);
    });

    let backend = CodingAgent::new(Agent::ClaudeCode, repository_root(&session));

    cx.background_executor()
        .spawn({
            let handoff = Arc::clone(&handoff);
            let cancel = Arc::clone(&cancel);
            async move {
                let events = Channel {
                    handoff: Arc::clone(&handoff),
                    cancel,
                };
                let result = session::run_review(&session, &backend, &events).map(|run| Outcome {
                    findings: run.findings,
                    unreviewed: run.excluded.into_iter().chain(run.omitted).collect(),
                });
                lock(&handoff).result = Some(result);
            }
        })
        .detach();

    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL_INTERVAL).await;

            let (detail, result) = {
                let mut state = lock(&handoff);
                (state.detail.take(), state.result.take())
            };

            let published = window.update(cx, |_, _window, cx| {
                review_view.update(cx, |review, cx| {
                    if let Some(detail) = detail {
                        review.review_progress(detail, cx);
                    }
                    match result {
                        None => false,
                        Some(Ok(outcome)) => {
                            review.review_finished(outcome.findings, outcome.unreviewed, cx);
                            true
                        }
                        Some(Err(error)) => {
                            review.review_failed(error.to_string(), error.remediation(), cx);
                            true
                        }
                    }
                })
            });

            // The window closing ends the run's reporting; the background task
            // notices the cancellation flag or finishes into a handoff nobody reads.
            match published {
                Ok(true) | Err(_) => break,
                Ok(false) => {}
            }
        }
    })
    .detach();
}

/// Where the backend runs, so relative paths in the diff resolve.
///
/// A session without a repository cannot reach here: `run_review` refuses one,
/// because there would be no anchors to validate findings against.
fn repository_root(session: &ReviewSession) -> std::path::PathBuf {
    match session.source() {
        domain::SessionSource::LocalComparison {
            repository_root, ..
        }
        | domain::SessionSource::GitHubPullRequest {
            repository_root, ..
        } => repository_root.clone(),
        domain::SessionSource::Demo => std::path::PathBuf::from("."),
    }
}

fn lock(handoff: &Arc<Mutex<Handoff>>) -> std::sync::MutexGuard<'_, Handoff> {
    handoff.lock().unwrap_or_else(PoisonError::into_inner)
}
