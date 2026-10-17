use std::{
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU32, Ordering},
        Mutex,
    },
};

use crate::engine::{Engine, Key, KeyOutcome};
use windows::{
    core::{implement, Error, Interface, Result, GUID, HRESULT},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_POINTER, S_FALSE, S_OK},
        System::Com::{IClassFactory, IClassFactory_Impl},
        UI::TextServices::{
            ITfContext, ITfEditSession, ITfEditSession_Impl, ITfKeyEventSink, ITfKeyEventSink_Impl,
            ITfKeystrokeMgr, ITfRange, ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
            TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION,
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
    engine: Engine,
    composition_range: Option<ITfRange>,
}

#[implement(ITfTextInputProcessor, ITfKeyEventSink)]
struct TsfService {
    state: std::sync::Arc<Mutex<ServiceState>>,
}

impl TsfService {
    fn new() -> Self {
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self {
            state: std::sync::Arc::new(Mutex::new(ServiceState::default())),
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

/// A write operation executed only while TSF grants the edit cookie.
#[implement(ITfEditSession)]
struct EditSession {
    context: ITfContext,
    state: std::sync::Arc<Mutex<ServiceState>>,
    text: Option<String>,
    keep_composition: bool,
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, edit_cookie: u32) -> Result<()> {
        use windows::core::PCWSTR;
        // Do not hold service state across TSF calls: text changes can re-enter callbacks.
        let existing_range = self.state.lock().map_err(|_| Error::from(E_FAIL))?.composition_range.clone();
        let mut range = if let Some(range) = existing_range {
            range
        } else {
            let mut selection = [TF_SELECTION::default()];
            let mut fetched = 0u32;
            unsafe { self.context.GetSelection(edit_cookie, 0, 1, selection.as_mut_ptr(), &mut fetched)?; }
            if fetched == 0 { return Err(Error::from(E_FAIL)); }
            selection[0].range.clone().ok_or_else(|| Error::from(E_FAIL))?
        };
        let text = self.text.as_deref().unwrap_or("");
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe { range.SetText(edit_cookie, 0, PCWSTR(wide.as_ptr()), wide.len() as i32)?; }
        let mut state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
        if self.keep_composition { state.composition_range = Some(range); } else { state.composition_range = None; }
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
        let key = key_from_virtual_key(wparam.0 as u32);
        let state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
        Ok(BOOL(state.engine.test_key(key) as i32))
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
        context: Option<&ITfContext>,
        wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        let Some(context) = context else { return Ok(BOOL(0)); };
        let key = key_from_virtual_key(wparam.0 as u32);
        let (client_id, engine_before, outcome) = {
            let mut state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
            if !state.engine.test_key(key) {
                return Ok(BOOL(0));
            }
            let engine_before = state.engine.clone();
            let outcome = state.engine.handle_key(key);
            (state.client_id, engine_before, outcome)
        };
        let (text, keep_composition, consumed) = match outcome {
            KeyOutcome::Update(text) => (Some(text), true, true),
            KeyOutcome::Commit(text) => (Some(text), false, true),
            KeyOutcome::Cancel => (Some(String::new()), false, true),
            KeyOutcome::Consumed => (None, false, true),
            KeyOutcome::PassThrough => (None, false, false),
        };
        if let Some(text) = text {
            if client_id == 0 { return Ok(BOOL(0)); }
            let session: ITfEditSession = EditSession {
                context: context.clone(),
                state: self.state.clone(),
                text: Some(text),
                keep_composition,
            }.into();
            let mut session_result = S_OK;
            let request_result = unsafe {
                context.RequestEditSession(
                    client_id,
                    &session,
                    TF_ES_SYNC | TF_ES_READWRITE,
                    &mut session_result,
                )
            };
            if request_result.is_err() || session_result.is_err() {
                // Keep the engine in sync with the document if TSF denies the edit.
                if let Ok(mut state) = self.state.lock() {
                    state.engine = engine_before;
                }
                request_result?;
                return Err(Error::from(session_result));
            }
        }
        Ok(BOOL(consumed as i32))
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
}

/// COM entry point called by COM activation for an in-process class factory.
///
/// # Safety
/// COM callers must pass valid pointers according to the `DllGetClassObject` ABI.
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
///
/// # Safety
/// This function has no pointer arguments and may be called by the COM loader.
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
}

fn key_from_virtual_key(vk: u32) -> Key {
    match vk {
        0x08 => Key::Backspace,
        0x1b => Key::Escape,
        0x25 | 0x26 => Key::Up,
        0x27 | 0x28 => Key::Down,
        0x20 => Key::Space,
        0x0d => Key::Enter,
        0x31..=0x39 => Key::Digit((vk - 0x30) as u8),
        0x41..=0x5a => Key::Character(char::from_u32(vk).unwrap_or_default()),
        _ => Key::Other,
    }
}
