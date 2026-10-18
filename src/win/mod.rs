//! Everything that talks to Windows. Only built on Windows.
//!
//! Where the Go version needed cgo thunks, hand-declared vtables and a
//! `syscall.NewCallback` per method, this side uses the `windows` crate:
//! `#[implement]` generates the vtables and the reference counting, and the TSF
//! interfaces are called directly. There is no C in this crate.

pub(crate) mod composition;
pub(crate) mod dll;
pub(crate) mod presenter;
pub(crate) mod registration;
pub(crate) mod service;

#[cfg(test)]
mod fake_tsf;
#[cfg(test)]
mod tests;

use windows::core::{Result, GUID};
use windows::Win32::Foundation::{E_FAIL, HMODULE};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};

use crate::diag::guarded;

pub(crate) fn guid(value: u128) -> GUID {
    GUID::from_u128(value)
}

/// Handle of this DLL (not of the host executable), found from the address of
/// code inside it.
pub(crate) fn this_module() -> Result<HMODULE> {
    let mut module = HMODULE::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(this_module as *const () as *const u16),
            &mut module,
        )?;
    }
    Ok(module)
}

/// Runs a COM callback that reports through `Result<()>`. A panic becomes
/// `E_FAIL` instead of unwinding into the host (see `diag::guarded`).
pub(crate) fn run_hr(name: &'static str, f: impl FnOnce() -> Result<()>) -> Result<()> {
    guarded(name, f).unwrap_or_else(|_| Err(E_FAIL.into()))
}
