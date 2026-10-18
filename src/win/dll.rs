//! The four exports every COM in-process server provides.
//!
//! Port of the exports in `tip/main.go`. Each one runs under `diag::guarded`: a
//! panic must not unwind into the host process, and since Rust 1.81 a panic out of
//! an `extern "system"` function would abort it.

use std::ffi::c_void;

use windows::core::{Interface, GUID, HRESULT};
use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_POINTER, S_FALSE};
use windows::Win32::System::Com::IClassFactory;

use super::registration::run_registration;
use super::service::{log_loaded, ClassFactory};
use crate::diag::guarded;
use crate::guids::{guid_text, CLSID_TEXT_SERVICE};
use crate::tip_log;

fn run_hresult(name: &'static str, f: impl FnOnce() -> HRESULT) -> HRESULT {
    guarded(name, f).unwrap_or(E_FAIL)
}

/// Hands TSF the class factory for this service's CLSID.
///
/// # Safety
/// `rclsid` and `riid` must be null or point to GUIDs, and `ppv` must be null or
/// point to writable storage for one pointer, as COM requires of every caller.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    run_hresult("DllGetClassObject", || {
        log_loaded();
        if ppv.is_null() {
            tip_log!("DllGetClassObject E_POINTER: null out");
            return E_POINTER;
        }
        unsafe { *ppv = std::ptr::null_mut() };
        if rclsid.is_null() || riid.is_null() {
            tip_log!("DllGetClassObject E_POINTER: null clsid or iid");
            return E_POINTER;
        }
        let clsid = unsafe { *rclsid }.to_u128();
        tip_log!("DllGetClassObject clsid={} iid={}", guid_text(clsid), guid_text(unsafe { *riid }.to_u128()));
        if clsid != CLSID_TEXT_SERVICE {
            tip_log!("DllGetClassObject CLASS_E_CLASSNOTAVAILABLE");
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = ClassFactory.into();
        let hr = unsafe { factory.query(riid, ppv) };
        tip_log!("DllGetClassObject query hr=0x{:08x} out={:?}", hr.0 as u32, unsafe { *ppv });
        hr
    })
}

/// The DLL must stay mapped: threads of this module (the candidate window, the
/// dump watcher) may still be running, and COM objects handed to TSF are not
/// counted here, so claiming "unloadable" could unmap live code.
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    run_hresult("DllRegisterServer", || run_registration(false))
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    run_hresult("DllUnregisterServer", || run_registration(true))
}
