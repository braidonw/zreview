//! Driving a review run without blocking the UI thread.
//!
//! The same shape as [`crate::loading`], and for the same reason: a review is
//! subprocess work that takes minutes, so it runs on the background executor while
//! a foreground task publishes what it reports. The difference is that a review can
//! be abandoned, so the cancellation flag is created here and handed to both sides.
//! The reviewer sets it, the backend polls it.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use app::{Handoff, SessionModel, lock};
use domain::{Findings, ReviewError, ReviewEventSink, ReviewProgress, ReviewSession};
use gpui::{App, WindowHandle};
use review::{Agent, CodingAgent};
use ui::SessionView;

/// How often the foreground task publishes progress.
///
/// Matches the loader's cadence. A review reports far less often than that, so this
/// is about the delay before a line appears, not about how much is displayed.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A finished run, in the shape the model needs.
struct Outcome {
    findings: Findings,
    /// Files the review did not see, so a partial review cannot present itself as
    /// a complete one.
    unreviewed: Vec<String>,
}

/// Publishes progress into the handoff and reports cancellation back out.
struct Channel {
    handoff: Arc<Handoff<String, Result<Outcome, ReviewError>>>,
    cancel: Arc<AtomicBool>,
}

impl ReviewEventSink for Channel {
    fn progress(&self, progress: ReviewProgress) {
        self.handoff.publish(progress.to_string());
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Runs a review of `session` in the background and publishes it into `model`.
///
/// The session is cloned rather than borrowed. The background thread must not hold
/// anything the UI thread is also mutating, and a snapshot's files are behind `Arc`
/// so the copy is cheap.
///
/// The model is passed in rather than reached through `window`, because this is
/// called from inside an update of the window's root view. Going back in through
/// the handle would be a second update of the same view, which panics. Only the
/// polling task below, which runs on its own, may use the handle.
pub fn spawn(
    window: WindowHandle<SessionView>,
    model: Arc<Mutex<SessionModel>>,
    session: ReviewSession,
    cx: &mut App,
) {
    let handoff = Handoff::new();
    let cancel = Arc::new(AtomicBool::new(false));

    // Told before the work starts, so the panel shows a running review immediately
    // rather than after the first progress report. Whoever asked for the review
    // repaints once this returns.
    lock(&model).review_started(Arc::clone(&cancel));

    let backend = CodingAgent::new(Agent::ClaudeCode, repository_root(&session));

    cx.background_executor()
        .spawn({
            let handoff = Arc::clone(&handoff);
            async move {
                let events = Channel {
                    handoff: Arc::clone(&handoff),
                    cancel,
                };
                let result = session::run_review(&session, &backend, &events).map(|run| Outcome {
                    findings: run.findings,
                    unreviewed: run.excluded.into_iter().chain(run.omitted).collect(),
                });
                handoff.finish(result);
            }
        })
        .detach();

    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL_INTERVAL).await;

            let (detail, result) = handoff.poll();

            let published = window.update(cx, |_view, _window, cx| {
                let (finished, moved) = {
                    let mut model = lock(&model);
                    let moved = detail.is_some_and(|detail| model.review_progress(detail));
                    let finished = match result {
                        None => false,
                        Some(Ok(outcome)) => {
                            model.review_finished(outcome.findings, outcome.unreviewed);
                            true
                        }
                        Some(Err(error)) => {
                            model.review_failed(error.to_string(), error.remediation());
                            true
                        }
                    };
                    (finished, moved)
                };
                // Polled ten times a second, so a repaint has to be earned: a
                // progress line that has not moved is not worth a frame.
                if finished || moved {
                    cx.notify();
                }
                finished
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
