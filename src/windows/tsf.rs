use std::{
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU32, Ordering},
        Mutex,
    },
};

use windows::{
    core::{implement, Error, Interface, Result, GUID, HRESULT},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_POINTER, S_FALSE, S_OK},
        System::Com::{IClassFactory, IClassFactory_Impl},
        UI::TextServices::{
            ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr,
            ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
        },
    },
};
use windows_core::IUnknownImpl;

/// CLSID used by the source Go TSF implementation.
pub const CLSID_PORT_RUST_IME: GUID = GUID::from_u128(0xc2d4a9b1_7f63_4a85_9e20_4b6d8c1f3072);
/// Language-profile GUID used by the source Go TSF implementation.
pub const GUID_PORT_RUST_IME_PROFILE: GUID =
    GUID::from_u128(0xe731b5d4_2a90_4f68_ac13_7d5e9b2046f1);

static LIVE_OBJECTS: AtomicU32 = AtomicU32::new(0);
static SERVER_LOCKS: AtomicU32 = AtomicU32::new(0);

impl ClassFactory {
    fn new() -> Self {
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ClassFactory {
    fn drop(&mut self) {
        LIVE_OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

#[implement(IClassFactory)]
struct ClassFactory;

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&windows::core::IUnknown>,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> Result<()> {
        if out.is_null() || iid.is_null() {
            return Err(Error::from(E_POINTER));
        }
        unsafe { *out = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(Error::from(HRESULT(0x80040110u32 as i32))); // CLASS_E_NOAGGREGATION
        }

        let tip: ITfTextInputProcessor = TsfService::new().into();
        let hr = unsafe { (Interface::vtable(&tip).base__.QueryInterface)(tip.as_raw(), iid, out) };
        if hr.is_err() {
            Err(Error::from(hr))
        } else {
            Ok(())
        }
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            SERVER_LOCKS.fetch_add(1, Ordering::SeqCst);
        } else {
            loop {
                let current = SERVER_LOCKS.load(Ordering::SeqCst);
                if current == 0
                    || SERVER_LOCKS
                        .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    break;
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct ServiceState {
    manager: Option<ITfKeystrokeMgr>,
    client_id: u32,
    active: bool,
}

#[implement(ITfTextInputProcessor, ITfKeyEventSink)]
struct TsfService {
    state: Mutex<ServiceState>,
}

impl TsfService {
    fn new() -> Self {
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self {
            state: Mutex::new(ServiceState::default()),
        }
    }
}

impl Default for TsfService {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TsfService {
    fn drop(&mut self) {
        LIVE_OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ITfTextInputProcessor_Impl for TsfService_Impl {
    fn Activate(&self, thread_mgr: Option<&ITfThreadMgr>, client_id: u32) -> Result<()> {
        let thread_mgr = thread_mgr.ok_or_else(|| Error::from(E_POINTER))?;
        let keystroke_mgr: ITfKeystrokeMgr = thread_mgr.cast()?;
        let sink: ITfKeyEventSink = self.to_interface();

        {
            let state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
            if state.active {
                return Err(Error::from(HRESULT(0x8000FFFFu32 as i32))); // E_UNEXPECTED
            }
        }

        // Advise can re-enter the sink; never hold the state mutex across COM calls.
        unsafe {
            keystroke_mgr.AdviseKeyEventSink(client_id, &sink, BOOL(1))?;
        }
        let mut state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
        state.manager = Some(keystroke_mgr);
        state.client_id = client_id;
        state.active = true;
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        let (manager, client_id) = {
            let state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
            if !state.active {
                return Ok(());
            }
            (state.manager.clone(), state.client_id)
        };

        // Unadvise can wait for callbacks that need this mutex. Release it before
        // calling TSF, and retain the registration state if unadvising fails.
        if let Some(manager) = manager {
            unsafe {
                manager.UnadviseKeyEventSink(client_id)?;
            }
        }

        let mut state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
        state.manager = None;
        state.client_id = 0;
        state.active = false;
        Ok(())
    }
}

impl ITfKeyEventSink_Impl for TsfService_Impl {
    fn OnSetFocus(&self, _foreground: BOOL) -> Result<()> {
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _context: Option<&ITfContext>,
        _wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        // Until edit sessions can safely write the document, pass all keys
        // through rather than claiming input that would otherwise be lost.
        Ok(BOOL(0))
    }

    fn OnTestKeyUp(
        &self,
        _context: Option<&ITfContext>,
        _wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn OnKeyDown(
        &self,
        _context: Option<&ITfContext>,
        _wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn OnKeyUp(
        &self,
        _context: Option<&ITfContext>,
        _wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn OnPreservedKey(&self, _context: Option<&ITfContext>, _guid: *const GUID) -> Result<BOOL> {
        Ok(BOOL(0))
    }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_match_the_go_tip_source() {
        assert_eq!(
            CLSID_PORT_RUST_IME,
            GUID::from_u128(0xc2d4a9b1_7f63_4a85_9e20_4b6d8c1f3072)
        );
        assert_eq!(
            GUID_PORT_RUST_IME_PROFILE,
            GUID::from_u128(0xe731b5d4_2a90_4f68_ac13_7d5e9b2046f1)
        );
    }

    #[test]
    fn virtual_key_mapping_is_stable() {
        assert_eq!(vk_to_key(0x4e), Key::Character('N'));
        assert_eq!(vk_to_key(0x32), Key::Digit(2));
        assert_eq!(vk_to_key(0x1b), Key::Escape);
        assert_eq!(vk_to_key(0x70), Key::Other);
    }
}
