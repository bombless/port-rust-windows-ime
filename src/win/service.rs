//! The text service: `ITfTextInputProcessorEx` plus `ITfKeyEventSink`.
//!
//! Port of `tip/main.go`. The Go code kept a COM object per interface
//! (`tipComObj`, `legacyTipComObj`, `keySinkComObj`) because go-com could only
//! express one vtable per object. Here a single `#[implement]` object carries all
//! of them: `ITfTextInputProcessorEx` inherits `ITfTextInputProcessor`, so
//! `QueryInterface` for either returns the same pointer, as it would in C++.
//!
//! Threading: the class is registered `ThreadingModel=Apartment`, so every TSF
//! callback arrives on the one thread that created the object. State therefore
//! uses `Cell`/`RefCell`, and the thing to defend against is *re-entrancy*, not
//! contention: an application can call back into the key sink while we are
//! inside one of its edit sessions. Go serialized key handling with a mutex and
//! carefully released it before the one call known to re-enter. A mutex held on
//! the same thread would simply deadlock the host, so here a busy flag turns a
//! re-entrant key callback into "not eaten" instead.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};

use windows::core::{implement, IUnknownImpl, Interface, Ref, Result, BOOL, GUID};
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, LPARAM, WPARAM};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_LWIN, VK_RWIN};
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfContext, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfRange,
    ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfTextInputProcessor_Impl, ITfThreadMgr,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use super::presenter::WindowPresenter;
use crate::diag::{guarded, pause_at, start_dump_watcher, thread_id, write_dump};
use crate::engine::key_name;
use crate::guids::{guid_text, CLSID_TEXT_SERVICE};
use crate::session::{key_decision, win_space_bypass_with, Session};
use crate::tip_log;

/// Whether either Windows key is currently down. `GetKeyState` is a user32
/// export; looking it up in the wrong DLL was a first-keystroke panic in Go.
pub(crate) fn win_held() -> bool {
    [VK_LWIN, VK_RWIN].iter().any(|vk| unsafe { GetKeyState(i32::from(vk.0)) } < 0)
}

pub(crate) fn win_space_bypass(vk: usize) -> bool {
    win_space_bypass_with(vk, win_held)
}

/// DPI of the window the user is typing into; 96 when unknown.
pub(crate) fn foreground_dpi() -> u32 {
    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        return 96;
    }
    match unsafe { GetDpiForWindow(window) } {
        0 => 96,
        dpi => dpi,
    }
}

/// Everything one text-service instance owns. Shared (`Rc`) with the edit
/// sessions it creates and, weakly, with the composition sink.
pub(crate) struct Core {
    pub session: Session,
    /// The composition on screen, if any. Dropping it releases the reference.
    pub composition: RefCell<Option<ITfComposition>>,
    /// The context that composition lives in. TSF contexts are reference counted,
    /// so holding one is safe; the Go version kept a bare pointer instead.
    pub composition_context: RefCell<Option<ITfContext>>,
    /// A collapsed clone of the committed range, waiting for the SetSelection
    /// that moves the caret behind the text (see `composition::place_pending_caret`).
    pub pending_caret: RefCell<Option<ITfRange>>,
    pub client_id: Cell<u32>,
    pub thread_mgr: RefCell<Option<ITfThreadMgr>>,
    pub sink_advised: Cell<bool>,
    key_busy: Cell<bool>,
}

/// Marks key handling as running until dropped.
pub(crate) struct KeyBusy<'a>(&'a Cell<bool>);

impl Drop for KeyBusy<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl Core {
    pub fn new(presenter: Box<dyn crate::candidate_ui::Presenter>) -> Core {
        Core {
            session: Session::new(presenter),
            composition: RefCell::new(None),
            composition_context: RefCell::new(None),
            pending_caret: RefCell::new(None),
            client_id: Cell::new(0),
            thread_mgr: RefCell::new(None),
            sink_advised: Cell::new(false),
            key_busy: Cell::new(false),
        }
    }

    /// Claims key handling. `None` means a key callback is already running on
    /// this thread, i.e. the application re-entered us; the caller must leave
    /// the key to the application rather than wait.
    pub fn enter_key(&self, what: &str) -> Option<KeyBusy<'_>> {
        if self.key_busy.replace(true) {
            tip_log!("TSF {what} re-entered while a key callback is running; leaving the key to the application");
            None
        } else {
            Some(KeyBusy(&self.key_busy))
        }
    }

    /// Runs one key through the engine and the document; reports whether the
    /// key was kept. The composition is only written for a key the engine keeps,
    /// and only with the context this very callback was handed.
    pub fn apply_key(self: &Rc<Self>, context: Option<&ITfContext>, vk: usize, lparam: isize) -> bool {
        if win_space_bypass(vk) {
            tip_log!("TSF OnKeyDown bypass Win+Space vk=0x{vk:x}");
            return false;
        }
        tip_log!(
            "StrategyTip OnKeyDown begin context={:?} vk=0x{vk:x} key={:?} lParam=0x{lparam:x} composition={} written={}",
            context.map(|c| c.as_raw()),
            key_name(vk),
            self.composition.borrow().is_some(),
            crate::diag::text_for_log(&self.session.written_composition()),
        );
        let reply = self.session.apply_key(vk);
        let consume = key_decision(reply.as_ref());
        tip_log!(
            "StrategyTip OnKeyDown reply vk=0x{vk:x} replied={} consume={consume} {}",
            reply.is_some(),
            reply.as_ref().map(|r| r.describe()).unwrap_or_default()
        );
        if let Some(context) = context {
            match (&reply, consume) {
                (Some(reply), true) => {
                    let caret = self.apply_composition(context, reply);
                    self.session.notify_candidates(reply, caret, foreground_dpi());
                }
                _ => self.forget_composition(context),
            }
        }
        consume
    }

    /// Puts the list back when focus returns to the application that still owns
    /// the composition.
    pub fn restore_candidates(&self) {
        self.session
            .restore_candidates(self.composition.borrow().is_some(), self.composition_context.borrow().is_some());
    }
}

/// The text service COM object.
#[implement(ITfTextInputProcessorEx, ITfKeyEventSink)]
pub(crate) struct TextService {
    pub core: Rc<Core>,
}

impl TextService {
    pub fn new() -> TextService {
        TextService { core: Rc::new(Core::new(Box::new(WindowPresenter::new()))) }
    }

    /// For tests: a service whose candidate list has no window behind it.
    #[cfg(test)]
    pub fn headless() -> TextService {
        TextService { core: Rc::new(Core::new(Box::new(crate::candidate_ui::NullPresenter))) }
    }
}

impl TextService_Impl {
    fn activate(&self, thread_mgr: Option<&ITfThreadMgr>, client_id: u32) -> Result<()> {
        // Reachability here decides whether any further logging exists at all, so
        // it comes before the first thing that can fail.
        tip_log!("StrategyTip Activate enter threadMgr={:?} clientId={client_id}", thread_mgr.map(|m| m.as_raw()));
        pause_at("Activate");
        let Some(thread_mgr) = thread_mgr else {
            tip_log!("StrategyTip Activate E_POINTER: no ITfThreadMgr");
            return Err(E_POINTER.into());
        };
        let keystrokes: ITfKeystrokeMgr = thread_mgr.cast()?;
        let sink: ITfKeyEventSink = self.to_interface();
        if let Err(error) = unsafe { keystrokes.AdviseKeyEventSink(client_id, &sink, true) } {
            tip_log!("StrategyTip AdviseKeyEventSink failed hr=0x{:08x}", error.code().0 as u32);
            write_dump("AdviseKeyEventSink failed");
            return Err(error);
        }
        let core = &self.core;
        *core.thread_mgr.borrow_mut() = Some(thread_mgr.clone());
        core.client_id.set(client_id);
        core.sink_advised.set(true);
        core.session.candidate_ui.set_enabled(true);
        tip_log!("StrategyTip key sink advised clientId={client_id}; {}", crate::session::dictionary_summary());
        Ok(())
    }

    fn deactivate(&self) -> Result<()> {
        let core = &self.core;
        tip_log!(
            "StrategyTip Deactivate enter sinkAdvised={} clientId={}",
            core.sink_advised.get(),
            core.client_id.get()
        );
        core.session.deactivate();
        pause_at("Deactivate");
        let thread_mgr = core.thread_mgr.borrow_mut().take();
        match thread_mgr {
            Some(thread_mgr) if core.sink_advised.get() => {
                let result = thread_mgr
                    .cast::<ITfKeystrokeMgr>()
                    .and_then(|keystrokes| unsafe { keystrokes.UnadviseKeyEventSink(core.client_id.get()) });
                let hr = result.as_ref().err().map_or(0, |e| e.code().0);
                tip_log!("StrategyTip UnadviseKeyEventSink hr=0x{:08x}", hr as u32);
                if result.is_err() {
                    write_dump("UnadviseKeyEventSink failed");
                }
                core.sink_advised.set(false);
            }
            _ => {
                // Activation never completed, so there is nothing to unadvise.
                // Saying so keeps a missing UnadviseKeyEventSink from looking like
                // a silent leak.
                tip_log!("StrategyTip Deactivate had no advised sink to retire");
            }
        }
        // A composition on screen belongs to a document that is going away; take
        // it down and drop our references so a dead ITfComposition is never used
        // by a later activation. This runs after the sink is retired, so no key
        // callback can be in flight any more.
        let context = core.composition_context.borrow().clone();
        if core.composition.borrow().is_some() && core.client_id.get() != 0 {
            if let Some(context) = &context {
                core.end_composition(Some(context));
            }
        }
        core.drop_composition();
        core.drop_pending_caret();
        *core.composition_context.borrow_mut() = None;
        Ok(())
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> Result<()> {
        super::run_hr("ITfTextInputProcessor.Activate", || self.activate(ptim.as_ref(), tid))
    }

    fn Deactivate(&self) -> Result<()> {
        super::run_hr("ITfTextInputProcessor.Deactivate", || self.deactivate())
    }
}

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(&self, ptim: Ref<ITfThreadMgr>, tid: u32, dwflags: u32) -> Result<()> {
        super::run_hr("ITfTextInputProcessorEx.ActivateEx", || {
            tip_log!(
                "StrategyTip ActivateEx flags=0x{dwflags:08x} secure={} uiElementOnly={} noActivateTip={} console={} wow16={}",
                dwflags & 0x2 != 0,
                dwflags & 0x4 != 0,
                dwflags & 0x1 != 0,
                dwflags & 0x40 != 0,
                dwflags & 0x10 != 0
            );
            let result = self.activate(ptim.as_ref(), tid);
            tip_log!(
                "StrategyTip ActivateEx result hr=0x{:08x}",
                result.as_ref().err().map_or(0, |e| e.code().0) as u32
            );
            result
        })
    }
}

/// Reports what a key callback decided, so a key the engine could not consume
/// shows up in the log as a decision rather than as silence.
fn eat(callback: &str, eaten: bool) -> bool {
    tip_log!("TSF {callback} -> eaten={eaten}");
    eaten
}

/// Runs a key callback that answers "was the key eaten".
///
/// On a panic the key is reported as *not* eaten with `S_OK`, so the application
/// simply gets its key. The Go guard answered `E_FAIL` here; but a failing HRESULT
/// from a key sink is the kind of thing that gets a text service dropped, and a
/// bug in the engine should cost one keystroke, not the input method.
fn key_callback(name: &'static str, f: impl FnOnce() -> bool) -> Result<BOOL> {
    Ok(BOOL::from(guarded(name, f).unwrap_or(false)))
}

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, fforeground: BOOL) -> Result<()> {
        super::run_hr("ITfKeyEventSink.OnSetFocus", || {
            tip_log!("TSF OnSetFocus foreground={} pid={}", fforeground.as_bool(), std::process::id());
            // The candidate list belongs to one document: TSF reports the focus
            // move here and the list has to go while another application owns the
            // text. Coming back puts it back while the composition is still alive.
            if fforeground.as_bool() {
                self.core.restore_candidates();
            } else {
                self.core.session.candidate_ui.suspend();
            }
            Ok(())
        })
    }

    fn OnTestKeyDown(&self, _pic: Ref<ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        key_callback("ITfKeyEventSink.OnTestKeyDown", || {
            let Some(_busy) = self.core.enter_key("OnTestKeyDown") else {
                return false;
            };
            if win_space_bypass(wparam.0) {
                tip_log!("TSF OnTestKeyDown bypass Win+Space vk=0x{:x}", wparam.0);
                return eat("OnTestKeyDown", false);
            }
            eat("OnTestKeyDown", self.core.session.predict_consume(wparam.0))
        })
    }

    fn OnTestKeyUp(&self, _pic: Ref<ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        key_callback("ITfKeyEventSink.OnTestKeyUp", || eat("OnTestKeyUp", false))
    }

    fn OnKeyDown(&self, pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        // Cloning takes a reference of our own, so the context stays valid for
        // the whole callback whatever the application does meanwhile.
        let context: Option<ITfContext> = pic.as_ref().cloned();
        key_callback("ITfKeyEventSink.OnKeyDown", || {
            let core = &self.core;
            let consume = match core.enter_key("OnKeyDown") {
                Some(busy) => {
                    let consume = core.apply_key(context.as_ref(), wparam.0, lparam.0);
                    drop(busy);
                    consume
                }
                None => false,
            };
            // SetSelection moves the caret behind committed text, but it notifies
            // the application, which can re-enter the TIP through a key callback.
            // The busy flag is released first so that callback is not mistaken
            // for a re-entrant one.
            if let Some(context) = &context {
                core.place_pending_caret(context);
            }
            eat("OnKeyDown", consume)
        })
    }

    fn OnKeyUp(&self, _pic: Ref<ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        key_callback("ITfKeyEventSink.OnKeyUp", || eat("OnKeyUp", false))
    }

    fn OnPreservedKey(&self, _pic: Ref<ITfContext>, rguid: *const GUID) -> Result<BOOL> {
        key_callback("ITfKeyEventSink.OnPreservedKey", || {
            let guid = if rguid.is_null() { 0 } else { unsafe { *rguid }.to_u128() };
            tip_log!("TSF OnPreservedKey guid={}", guid_text(guid));
            eat("OnPreservedKey", false)
        })
    }
}

/// The class factory TSF gets from `DllGetClassObject`.
#[implement(IClassFactory)]
pub(crate) struct ClassFactory {
    pub locks: AtomicI32,
}

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<windows::core::IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        super::run_hr("IClassFactory.CreateInstance", || {
            if ppvobject.is_null() {
                tip_log!("CreateInstance E_POINTER: null out");
                return Err(E_POINTER.into());
            }
            unsafe { *ppvobject = std::ptr::null_mut() };
            if riid.is_null() {
                tip_log!("CreateInstance E_POINTER: null iid");
                return Err(E_POINTER.into());
            }
            if !punkouter.is_null() {
                tip_log!("CreateInstance CLASS_E_NOAGGREGATION: TSF asked for aggregation");
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let object: windows::core::IUnknown = TextService::new().into();
            let hr = unsafe { object.query(riid, ppvobject) };
            // A wrong IID here is the classic silent failure: TSF drops the text
            // service without ever calling Activate, so nothing else would report it.
            tip_log!(
                "CreateInstance iid={} out={:?} hr=0x{:08x}",
                guid_text(unsafe { *riid }.to_u128()),
                unsafe { *ppvobject },
                hr.0 as u32
            );
            hr.ok()
        })
    }

    fn LockServer(&self, flock: BOOL) -> Result<()> {
        super::run_hr("IClassFactory.LockServer", || {
            if flock.as_bool() {
                self.locks.fetch_add(1, Ordering::SeqCst);
            } else if self.locks.load(Ordering::SeqCst) > 0 {
                self.locks.fetch_sub(1, Ordering::SeqCst);
            }
            tip_log!("LockServer lock={} serverLocks={}", flock.as_bool() as i32, self.locks.load(Ordering::SeqCst));
            Ok(())
        })
    }
}

/// Logged once, the first time TSF or a test asks for the class.
pub(crate) fn log_loaded() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        start_dump_watcher();
        tip_log!(
            "RustTip DLL loaded pid={} tid={} dumpWatch={} clsid={}",
            std::process::id(),
            thread_id(),
            crate::diag::dump_watch_path().display(),
            guid_text(CLSID_TEXT_SERVICE)
        );
    });
}

/// `E_FAIL` as the generic failure the guards report.
#[allow(dead_code)]
pub(crate) const FAILURE: windows::core::HRESULT = E_FAIL;
