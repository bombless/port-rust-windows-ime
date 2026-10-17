use std::{
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU32, Ordering},
        Mutex,
    },
};

use windows::{
    core::{implement, Error, HRESULT, Interface, Result, BOOL, GUID},
    Win32::{
        Foundation::{E_FAIL, E_POINTER, S_FALSE, S_OK},
        System::Com::{IClassFactory, IClassFactory_Impl},
        UI::TextServices::{
            ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr,
            ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
        },
    },
};

use crate::engine::{Engine, Key};

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
            let _ = SERVER_LOCKS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n > 0).then_some(n - 1)
            });
        }
        Ok(())
    }
}

#[derive(Default)]
struct ServiceState {
    manager: Option<ITfKeystrokeMgr>,
    client_id: u32,
    active: bool,
    engine: Engine,
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

        let mut state = self.0.state.lock().map_err(|_| Error::from(E_FAIL))?;
        if state.active {
            return Err(Error::from(HRESULT(0x8001010Du32 as i32))); // RPC_E_CANTCALLOUT_ININPUTSYNCCALL
        }

        unsafe {
            keystroke_mgr.AdviseKeyEventSink(client_id, &sink, BOOL(1))?;
        }
        state.manager = Some(keystroke_mgr);
        state.client_id = client_id;
        state.active = true;
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        let mut state = self.0.state.lock().map_err(|_| Error::from(E_FAIL))?;
        if !state.active {
            return Ok(());
        }

        // Keep the manager and active flag if TSF refuses to unadvise. This
        // preserves the only handle that can retry cleanup instead of silently
        // losing the registration state.
        if let Some(manager) = state.manager.as_ref() {
            unsafe { manager.UnadviseKeyEventSink(state.client_id)?; }
        }
        state.manager = None;
        state.client_id = 0;
        state.active = false;
        state.engine = Engine::new();
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
        wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        let state = self.0.state.lock().map_err(|_| Error::from(E_FAIL))?;
        Ok(BOOL(i32::from(state.engine.test_key(vk_to_key(wparam.0)))))
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
        wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        let mut state = self.0.state.lock().map_err(|_| Error::from(E_FAIL))?;
        let key = vk_to_key(wparam.0);
        if !state.engine.test_key(key) {
            return Ok(BOOL(0));
        }
        let _outcome = state.engine.handle_key(key);
        // Consumption/state transition is wired. Text composition/edit-session
        // integration is intentionally a separate next step.
        Ok(BOOL(1))
    }

    fn OnKeyUp(
        &self,
        _context: Option<&ITfContext>,
        _wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn OnPreservedKey(
        &self,
        _context: Option<&ITfContext>,
        _guid: *const GUID,
    ) -> Result<BOOL> {
        Ok(BOOL(0))
    }
}

fn vk_to_key(vk: usize) -> Key {
    match vk {
        0x08 => Key::Backspace,
        0x1b => Key::Escape,
        0x26 => Key::Up,
        0x28 => Key::Down,
        0x20 => Key::Space,
        0x0d => Key::Enter,
        0x31..=0x39 => Key::Digit((vk - 0x30) as u8),
        0x41..=0x5a => Key::Character(char::from_u32(vk as u32).unwrap_or_default()),
        _ => Key::Other,
    }
}

/// COM entry point called by COM activation for an in-process class factory.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    match catch_unwind(AssertUnwindSafe(|| {
        if out.is_null() {
            return E_POINTER;
        }
        *out = std::ptr::null_mut();
        if clsid.is_null() || iid.is_null() {
            return E_POINTER;
        }
        if *clsid != CLSID_PORT_RUST_IME {
            return HRESULT(0x80040111u32 as i32); // CLASS_E_CLASSNOTAVAILABLE
        }

        let factory: IClassFactory = ClassFactory::new().into();
        (Interface::vtable(&factory).base__.QueryInterface)(factory.as_raw(), iid, out)
    })) {
        Ok(hr) => hr,
        Err(_) => E_FAIL,
    }
}

/// Reports whether this DLL has outstanding COM objects or server locks.
#[no_mangle]
pub unsafe extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LIVE_OBJECTS.load(Ordering::SeqCst) == 0 && SERVER_LOCKS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
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
