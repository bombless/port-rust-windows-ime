pub mod engine;
#[cfg(windows)]
pub mod windows;
pub use engine::{Candidate, Engine, Key, KeyOutcome, Snapshot};
