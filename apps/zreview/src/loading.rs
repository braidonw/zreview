//! Driving a session load without blocking the UI thread.
//!
//! The window is already open when this starts, so everything here is about
//! getting a background result onto the foreground thread. `session::load` is
//! blocking subprocess work, so it runs on the background executor; the
//! foreground task wakes on a timer to publish the stage it reports and, once it
//! finishes, the session itself.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use domain::{LoadStage, ReviewSession, SessionFailure};
use gpui::{App, WindowHandle};
use session::SessionRequest;
use ui::SessionView;

/// How often the foreground task publishes progress.
///
/// Fast enough that a stage change looks immediate, slow enough to be invisible
/// next to the subprocesses being waited on.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// What the background load hands to the foreground task.
///
/// A lock rather than a channel keeps this to the standard library: the
/// foreground side only ever needs the *latest* stage, so there is nothing to
/// queue.
#[derive(Default)]
struct Handoff {
    stage: LoadStage,
    result: Option<Result<ReviewSession, SessionFailure>>,
}

/// Loads `request` in the background and publishes the outcome into `window`.
pub fn spawn(window: WindowHandle<SessionView>, request: SessionRequest, cx: &mut App) {
    let handoff = Arc::new(Mutex::new(Handoff::default()));

    cx.background_executor()
        .spawn({
            let handoff = Arc::clone(&handoff);
            async move {
                let result = session::load(&request, &|stage| {
                    lock(&handoff).stage = stage;
                });
                lock(&handoff).result = Some(result);
            }
        })
        .detach();

    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL_INTERVAL).await;

            let (stage, result) = {
                let mut handoff = lock(&handoff);
                (handoff.stage, handoff.result.take())
            };

            let published = window.update(cx, |view, window, cx| {
                if let Some(result) = result {
                    view.finish(result, window, cx);
                    true
                } else {
                    view.set_stage(stage.label(), cx);
                    false
                }
            });

            // Stop on completion, and also if the window has gone away.
            match published {
                Ok(false) => {}
                Ok(true) | Err(_) => break,
            }
        }
    })
    .detach();
}

/// Takes the lock, recovering from poisoning.
///
/// A panic while loading would poison the mutex; the guarded data is still
/// consistent, and refusing to show the outcome would be a worse failure than
/// the one that caused it.
fn lock(handoff: &Mutex<Handoff>) -> std::sync::MutexGuard<'_, Handoff> {
    handoff.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::TestAppContext;
    use ui::SessionView;

    use super::*;

    /// Lets the background load finish and the foreground task publish it.
    fn settle(cx: &mut TestAppContext) {
        for _ in 0..10 {
            cx.executor().advance_clock(POLL_INTERVAL);
            cx.run_until_parked();
        }
    }

    /// Covers the whole pipeline: background load, timer-driven publication, and
    /// the window ending up with a session it never blocked on.
    #[gpui::test]
    fn a_successful_load_reaches_the_window(cx: &mut TestAppContext) {
        cx.update(ui::init);
        let window = cx.add_window(|_window, cx| SessionView::loading("the fixture", cx));
        window
            .update(cx, |view, _window, _cx| assert!(view.is_loading()))
            .unwrap();

        cx.update(|cx| spawn(window, SessionRequest::Demo, cx));
        settle(cx);

        window
            .update(cx, |view, _window, _cx| {
                assert!(!view.is_loading(), "the session should have been published");
                assert!(view.failure().is_none());
            })
            .unwrap();
    }

    /// A failure has to travel the same path as a success, or a broken load would
    /// leave the reviewer on the loading screen forever.
    #[gpui::test]
    fn a_failed_load_reaches_the_window_too(cx: &mut TestAppContext) {
        cx.update(ui::init);
        let window = cx.add_window(|_window, cx| SessionView::loading("a bad request", cx));

        cx.update(|cx| {
            spawn(
                window,
                SessionRequest::LocalComparison {
                    // Not a repository, so loading fails.
                    repository: PathBuf::from("/nonexistent-zreview-test-path"),
                    base: "main".to_owned(),
                    head: "HEAD".to_owned(),
                },
                cx,
            );
        });
        settle(cx);

        window
            .update(cx, |view, _window, _cx| {
                assert!(!view.is_loading());
                let failure = view.failure().expect("the failure should be shown");
                assert!(!failure.summary.is_empty());
            })
            .unwrap();
    }
}
