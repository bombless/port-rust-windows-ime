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
        UI::{
            Input::KeyboardAndMouse::GetKeyState,
            TextServices::{
                ITfContext, ITfEditSession, ITfEditSession_Impl, ITfKeyEventSink,
                ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfRange, ITfTextInputProcessor,
                ITfTextInputProcessor_Impl, ITfThreadMgr, TF_ANCHOR_END, TF_ES_READWRITE,
                TF_ES_SYNC, TF_SELECTION,
            },
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
    english_mode: bool,
    shift_pending: bool,
    shift_used: bool,
    engine: Engine,
    composition_range: Option<ITfRange>,
}

#[implement(ITfTextInputProcessor, ITfKeyEventSink)]
struct TsfService {
    state: std::sync::Arc<Mutex<ServiceState>>,
}

impl TsfService {
    #[allow(clippy::arc_with_non_send_sync)]
    fn new() -> Self {
        LIVE_OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self {
            // This Arc shares state with a synchronous edit-session callback; it is
            // not used to move apartment-bound TSF interfaces across threads.
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
        // A range belongs to the document/context that created it. Never carry
        // it into a later activation, which may target a different document.
        state.composition_range = None;
        state.engine = Engine::default();
        crate::windows::candidate_ui::hide();
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
        // Do not hold service state across TSF calls: text changes can re-enter callbacks.
        let existing_range = self
            .state
            .lock()
            .map_err(|_| Error::from(E_FAIL))?
            .composition_range
            .clone();
        let range = if let Some(range) = existing_range {
            range
        } else {
            let mut selection = [TF_SELECTION::default()];
            let mut fetched = 0u32;
            unsafe {
                self.context
                    .GetSelection(edit_cookie, 0, &mut selection, &mut fetched)?;
            }
            if fetched == 0 {
                return Err(Error::from(E_FAIL));
            }
            selection[0]
                .range
                .as_ref()
                .ok_or_else(|| Error::from(E_FAIL))?
                .clone()
        };
        let text = self.text.as_deref().unwrap_or("");
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            range.SetText(edit_cookie, 0, &wide)?;

            // SetText changes the range contents but does not guarantee that the
            // document selection (caret) moves to the end of the inserted text.
            // Collapse a cloned range so the composition range itself remains
            // intact for subsequent updates, then explicitly place the selection.
            let caret_range = range.Clone()?;
            caret_range.Collapse(edit_cookie, TF_ANCHOR_END)?;
            let mut caret = [TF_SELECTION::default()];
            caret[0].range = std::mem::ManuallyDrop::new(Some(caret_range));
            let selection_result = self.context.SetSelection(edit_cookie, &caret);
            // TF_SELECTION stores its interface pointer in ManuallyDrop; release
            // our temporary range reference after TSF has copied the selection.
            std::mem::ManuallyDrop::drop(&mut caret[0].range);
            selection_result?;
        }
        let mut state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
        if self.keep_composition {
            state.composition_range = Some(range);
        } else {
            state.composition_range = None;
        }
        Ok(())
    }
}

/// Leave system/application shortcuts to the focused application. TSF asks
/// OnTestKeyDown before OnKeyDown, so apply the same modifier guard in both.
fn has_shortcut_modifier() -> bool {
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_LWIN: i32 = 0x5B;
    const VK_RWIN: i32 = 0x5C;
    [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(|vk| unsafe { GetKeyState(vk) } < 0)
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
        let vk = wparam.0 as u32;
        if is_shift_key(vk) { return Ok(BOOL(1)); }
        if has_shortcut_modifier() { return Ok(BOOL(0)); }
        let state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
        if state.shift_pending { return Ok(BOOL(1)); }
        if state.english_mode { return Ok(BOOL(0)); }
        let key = key_from_virtual_key(vk);
        Ok(BOOL(state.engine.test_key(key) as i32))
    }

    fn OnTestKeyUp(
        &self,
        _context: Option<&ITfContext>,
        wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL(is_shift_key(wparam.0 as u32) as i32))
    }

    fn OnKeyDown(
        &self,
        context: Option<&ITfContext>,
        wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        let vk = wparam.0 as u32;
        if is_shift_key(vk) {
            let mut state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
            state.shift_pending = true; state.shift_used = false;
            return Ok(BOOL(1));
        }
        {
            let mut state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
            if state.shift_pending {
                state.shift_used = true;
                if !state.english_mode {
                    state.english_mode = true;
                    state.engine = Engine::default();
                    crate::windows::candidate_ui::hide();
                    return Ok(BOOL(0));
                }
            }
            if state.english_mode { return Ok(BOOL(0)); }
        }
        let Some(context) = context else { return Ok(BOOL(0)); };
        if has_shortcut_modifier() { return Ok(BOOL(0)); }
        let key = key_from_virtual_key(vk);
        let (client_id, engine_before, outcome, snapshot) = {
            let mut state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
            if !state.engine.test_key(key) {
                return Ok(BOOL(0));
            }
            let engine_before = state.engine.clone();
            let outcome = state.engine.handle_key(key);
            let snapshot = state.engine.snapshot();
            (state.client_id, engine_before, outcome, snapshot)
        };
        let (text, keep_composition, consumed) = match outcome {
            KeyOutcome::Update(text) => (Some(text), true, true),
            KeyOutcome::Commit(text) => (Some(text), false, true),
            KeyOutcome::Cancel => (Some(String::new()), false, true),
            KeyOutcome::Consumed => (None, false, true),
            KeyOutcome::PassThrough => (None, false, false),
        };
        if let Some(text) = text {
            if client_id == 0 {
                if let Ok(mut state) = self.state.lock() {
                    state.engine = engine_before;
                }
                return Ok(BOOL(0));
            }
            let session: ITfEditSession = EditSession {
                context: context.clone(),
                state: self.state.clone(),
                text: Some(text),
                keep_composition,
            }
            .into();
            let request_result = unsafe {
                context.RequestEditSession(client_id, &session, TF_ES_SYNC | TF_ES_READWRITE)
            };
            match request_result {
                Err(error) => {
                    // Keep the engine in sync with the document if TSF denies the edit.
                    if let Ok(mut state) = self.state.lock() {
                        state.engine = engine_before;
                    }
                    return Err(error);
                }
                Ok(session_result) if session_result.is_err() => {
                    if let Ok(mut state) = self.state.lock() {
                        state.engine = engine_before;
                    }
                    return Err(Error::from(session_result));
                }
                Ok(_) => {}
            }
        }
        crate::windows::candidate_ui::update(&snapshot);
        Ok(BOOL(consumed as i32))
    }

    fn OnKeyUp(
        &self,
        _context: Option<&ITfContext>,
        wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        if is_shift_key(wparam.0 as u32) {
            let mut state = self.state.lock().map_err(|_| Error::from(E_FAIL))?;
            if state.shift_pending && !state.shift_used {
                state.english_mode = !state.english_mode;
                state.engine = Engine::default();
                crate::windows::candidate_ui::hide();
            }
            state.shift_pending = false; state.shift_used = false;
        }
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

fn is_shift_key(vk: u32) -> bool { matches!(vk, 0x10 | 0xA0 | 0xA1) }

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
    fn virtual_key_mapping_covers_ime_controls() {
        assert_eq!(key_from_virtual_key(0x08), Key::Backspace);
        assert_eq!(key_from_virtual_key(0x1b), Key::Escape);
        assert_eq!(key_from_virtual_key(0x25), Key::Up);
        assert_eq!(key_from_virtual_key(0x28), Key::Down);
        assert_eq!(key_from_virtual_key(0x20), Key::Space);
        assert_eq!(key_from_virtual_key(0x0d), Key::Enter);
        assert_eq!(key_from_virtual_key(0x31), Key::Digit(1));
        assert_eq!(key_from_virtual_key(0x39), Key::Digit(9));
        assert_eq!(key_from_virtual_key(0x41), Key::Character('A'));
        assert_eq!(key_from_virtual_key(0x5a), Key::Character('Z'));
        assert_eq!(key_from_virtual_key(0x70), Key::Other);
    }
}
