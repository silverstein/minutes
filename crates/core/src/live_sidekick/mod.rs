//! Surface-neutral live-assistance session state.
//!
//! The model in this module is owned by Minutes rather than any one UI or
//! provider. It deliberately separates typed user authority from transcript,
//! screen, and other meeting evidence.

mod engine;
mod provider;
mod session;

pub use engine::*;
pub use provider::*;
pub use session::*;
