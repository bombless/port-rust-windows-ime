//! A standalone TSF (Text Services Framework) pinyin input method in Rust.
//!
//! Port of `port-golang-windows-ime`. The crate is layered so that almost all
//! logic can be tested on any host:
//!
//! * [`engine`]       – dictionary, candidate generation, key reducer (pure)
//! * [`candidate_ui`] – candidate view model and the presenter seam (pure)
//! * [`session`]      – per-service state shared by the key path (pure)
//! * [`diag`]         – logging, panic guard, in-flight callback tracking
//! * [`guids`]        – CLSID / profile / category identities
//! * [`layout`]       – candidate-window text and placement (pure)
//! * `win`            – everything that talks to Windows (COM objects, edit
//!   sessions, registration, the candidate window); `cfg(windows)` only
//!
//! The DLL exports (`DllGetClassObject` and friends) live in `win::dll`.

pub mod candidate_ui;
pub mod diag;
pub mod engine;
pub mod guids;
pub mod layout;
pub mod session;

#[cfg(windows)]
mod win;
