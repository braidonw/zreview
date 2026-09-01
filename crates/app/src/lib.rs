//! What a review session does, with no window in it.
//!
//! Everything here is the orchestration between a reviewer's actions and the
//! domain, from which file is open and what has been drafted to how far a review
//! run has got and what a confirmed submission does to storage. A view calls these
//! methods and renders what comes back; effects are return values rather than
//! callbacks, so the same model drives any front end.
//!
//! The only seams out of this crate are the domain's two ports,
//! [`domain::ReviewStateSink`] and [`domain::ReviewSubmitter`]. Nothing here
//! knows about a database, a forge, or a UI framework.

mod handoff;
mod review;
mod session;

pub use handoff::Handoff;
pub use review::{FindingDisposition, ReviewModel, ReviewRunState};
pub use session::{PendingSend, SessionModel, SessionPhase, SubmissionState, lock};
