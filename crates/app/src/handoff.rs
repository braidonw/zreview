//! Getting a background worker's progress and result onto another thread.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// What a background worker hands to whoever is polling it.
///
/// A lock rather than a channel keeps this to the standard library. The polling
/// side only ever needs the latest progress, so there is nothing to queue.
/// Publishing overwrites, and polling takes.
pub struct Handoff<P, R> {
    state: Mutex<State<P, R>>,
}

struct State<P, R> {
    progress: Option<P>,
    result: Option<R>,
}

impl<P, R> Handoff<P, R> {
    /// Creates a handoff both sides can hold.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                progress: None,
                result: None,
            }),
        })
    }

    /// Reports how far the work has got, replacing any earlier report.
    pub fn publish(&self, progress: P) {
        self.lock().progress = Some(progress);
    }

    /// Reports the outcome. The work is over once this is called.
    pub fn finish(&self, result: R) {
        self.lock().result = Some(result);
    }

    /// Takes whatever has arrived since the last poll.
    #[must_use]
    pub fn poll(&self) -> (Option<P>, Option<R>) {
        let mut state = self.lock();
        (state.progress.take(), state.result.take())
    }

    /// Takes the lock, recovering from poisoning.
    ///
    /// A panic in the background work would poison the mutex; the guarded data is
    /// still consistent, and refusing to report the outcome would be a worse
    /// failure than the one that caused it.
    fn lock(&self) -> MutexGuard<'_, State<P, R>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
