//! What `ZReview` does, with no window in it.
//!
//! Home and one review sitting. Everything here is the orchestration between a
//! reviewer's actions and the domain, from which repositories Home lists and
//! which file is open to how far a review run has got and what a confirmed
//! submission does to storage. A view calls these methods and renders what comes
//! back; effects are return values rather than callbacks, so the same models
//! drive any front end.
//!
//! The only seams out of this crate are the domain's two ports,
//! [`domain::ReviewStateSink`] and [`domain::ReviewSubmitter`]. Nothing here
//! knows about a database, a forge, or a UI framework.

use std::sync::{Mutex, MutexGuard, PoisonError};

mod handoff;
mod home;
mod review;
mod session;

pub use handoff::Handoff;
pub use home::{
    CheckState, CheckStatus, FetchFailure, FetchedPullRequest, HomeGroup, HomeModel, HomeRow,
    HomeSearch, RefreshState, Refusal, RepositoryEntry, RepositoryFetch, RepositoryOutcome,
    ReviewDecision, ReviewStatus, SettingsWrite,
};
pub use review::{FindingDisposition, ReviewModel, ReviewRunState};
pub use session::{PendingSend, SessionModel, SessionPhase, SubmissionState};

/// Takes a model's lock, recovering from poisoning. The guarded state stays
/// consistent, and refusing to use it would be worse than the panic that
/// poisoned it.
pub fn lock<T>(model: &Mutex<T>) -> MutexGuard<'_, T> {
    model.lock().unwrap_or_else(PoisonError::into_inner)
}
