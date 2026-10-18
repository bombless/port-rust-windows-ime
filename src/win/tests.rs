//! Tests that need Windows: COM identity, the DLL exports, and the whole key path
//! against the fake TSF host in `fake_tsf`.
//!
//! These replace the Go repo's COM fixture tests and the native C vtable probe:
//! the vtables are generated now, so what is left to prove is that the *service*
//! answers the way TSF expects, and that an edit session really leaves the
//! document in the right state.

use std::ffi::c_void;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::core::{AsImpl, IUnknown, Interface, GUID};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_NOINTERFACE, E_POINTER, LPARAM, RECT, S_FALSE, S_OK, WPARAM,
};
use windows::Win32::System::Com::IClassFactory;
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, GetWindowRect, IsWindow, IsWindowVisible, PostMessageW, GWL_EXSTYLE, WM_CLOSE, WS_EX_NOACTIVATE,
    WS_EX_TOPMOST,
};

use super::fake_tsf::*;
use super::guid;
use super::presenter::WindowPresenter;
use super::service::{Core, TextService};
use crate::candidate_ui::{Presenter, Snapshot};
use crate::diag::test_support::LogRedirect;
use crate::engine::{candidates_for, Candidate};
use crate::guids::*;

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

// port of TestClosedInterfaceIdentities: the interfaces and categories the
// `windows` crate calls by these names must carry the values the Go port
// recorded from the SDK, because registration and QueryInterface depend on them.
#[test]
fn interface_and_category_identities_match_the_sdk_values() {
    assert_eq!(ITfTextInputProcessor::IID.to_u128(), IID_ITF_TEXT_INPUT_PROCESSOR);
    assert_eq!(ITfTextInputProcessorEx::IID.to_u128(), IID_ITF_TEXT_INPUT_PROCESSOR_EX);
    assert_eq!(ITfKeyEventSink::IID.to_u128(), IID_ITF_KEY_EVENT_SINK);
    assert_eq!(IClassFactory::IID.to_u128(), IID_ICLASSFACTORY);
    assert_eq!(IUnknown::IID.to_u128(), IID_IUNKNOWN);

    assert_eq!(guid(CAT_TIP_KEYBOARD), GUID_TFCAT_TIP_KEYBOARD);
    assert_eq!(guid(CAT_TIPCAP_COMLESS), GUID_TFCAT_TIPCAP_COMLESS);
    assert_eq!(guid(CAT_TIPCAP_IMMERSIVE_SUPPORT), GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT);
    assert_eq!(guid(CAT_TIPCAP_SECURE_MODE), GUID_TFCAT_TIPCAP_SECUREMODE);
    assert_eq!(guid(CAT_TIPCAP_SYSTRAY_SUPPORT), GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT);
    assert_eq!(guid(CAT_TIPCAP_UIELEMENT_ENABLED), GUID_TFCAT_TIPCAP_UIELEMENTENABLED);
}

// port of the QueryInterface/identity half of processor_com_fixture_test.go
#[test]
fn query_interface_answers_what_tsf_asks_for() {
    let service: ITfTextInputProcessorEx = TextService::headless().into();
    let unknown: IUnknown = service.cast().unwrap();

    // Ex inherits ITfTextInputProcessor, so both are the same object; the key
    // sink is a second interface on it.
    let base: ITfTextInputProcessor = service.cast().expect("ITfTextInputProcessor");
    let sink: ITfKeyEventSink = service.cast().expect("ITfKeyEventSink");
    let again: ITfTextInputProcessorEx = base.cast().expect("ITfTextInputProcessorEx from the base");

    // COM identity: every interface agrees on its IUnknown.
    for (name, other) in [
        ("base", base.cast::<IUnknown>().unwrap()),
        ("sink", sink.cast::<IUnknown>().unwrap()),
        ("ex", again.cast::<IUnknown>().unwrap()),
    ] {
        assert_eq!(other.as_raw(), unknown.as_raw(), "{name} must share the object's IUnknown");
    }

    // Interfaces the object does not implement are refused cleanly.
    assert_eq!(service.cast::<IClassFactory>().unwrap_err().code(), E_NOINTERFACE);
    assert_eq!(service.cast::<ITfComposition>().unwrap_err().code(), E_NOINTERFACE);
    assert_eq!(service.cast::<ITfEditSession>().unwrap_err().code(), E_NOINTERFACE);
}

// port of the lifetime half: AddRef/Release balance and the object dies with its last reference
#[test]
fn reference_counting_balances_and_the_object_dies_with_its_last_reference() {
    let core_weak = {
        let service: ITfTextInputProcessorEx = TextService::headless().into();
        let core = unsafe { AsImpl::<TextService>::as_impl(&service) }.core.clone();
        let weak = Rc::downgrade(&core);
        drop(core);
        let extra = service.clone();
        let sink: ITfKeyEventSink = service.cast().unwrap();
        drop(service);
        assert!(weak.upgrade().is_some(), "other references keep the service alive");
        drop(extra);
        assert!(weak.upgrade().is_some(), "the sink interface keeps it alive too");
        drop(sink);
        weak
    };
    assert!(core_weak.upgrade().is_none(), "the last Release destroys the service");
}

// ---------------------------------------------------------------------------
// The DLL exports
// ---------------------------------------------------------------------------

fn get_class_object(clsid: &GUID, iid: &GUID) -> (windows::core::HRESULT, *mut c_void) {
    let mut out: *mut c_void = std::ptr::null_mut();
    let hr = unsafe { super::dll::DllGetClassObject(clsid, iid, &mut out) };
    (hr, out)
}

#[test]
fn dll_get_class_object_serves_the_class_factory() {
    let _log = LogRedirect::new("dll");
    let (hr, raw) = get_class_object(&guid(CLSID_TEXT_SERVICE), &IClassFactory::IID);
    assert_eq!(hr, S_OK);
    let factory = unsafe { IClassFactory::from_raw(raw) };

    let service: ITfTextInputProcessorEx =
        unsafe { factory.CreateInstance(None::<&IUnknown>) }.expect("CreateInstance");
    let _sink: ITfKeyEventSink = service.cast().expect("a created service carries the key sink");

    // The service is also reachable as plain IUnknown and the base interface.
    let _: IUnknown = unsafe { factory.CreateInstance(None::<&IUnknown>) }.unwrap();
    let _: ITfTextInputProcessor = unsafe { factory.CreateInstance(None::<&IUnknown>) }.unwrap();

    // Aggregation is refused, and a wrong IID is E_NOINTERFACE, not a crash.
    let outer: IUnknown = TextService::headless().into();
    let refused = unsafe { factory.CreateInstance::<_, ITfTextInputProcessorEx>(&outer) };
    assert_eq!(refused.unwrap_err().code(), CLASS_E_NOAGGREGATION);
    let wrong = unsafe { factory.CreateInstance::<_, ITfComposition>(None::<&IUnknown>) };
    assert_eq!(wrong.unwrap_err().code(), E_NOINTERFACE);

    // LockServer is balanced and never goes negative.
    unsafe {
        factory.LockServer(true).unwrap();
        factory.LockServer(false).unwrap();
        factory.LockServer(false).unwrap();
    }
}

#[test]
fn dll_get_class_object_rejects_bad_requests() {
    let _log = LogRedirect::new("dll-bad");
    let (hr, raw) = get_class_object(&guid(0x1111_2222_3333_4444_5555_6666_7777_8888), &IClassFactory::IID);
    assert_eq!((hr, raw.is_null()), (CLASS_E_CLASSNOTAVAILABLE, true), "a CLSID that is not ours");

    let (hr, raw) = get_class_object(&guid(CLSID_TEXT_SERVICE), &guid(0x1111_2222_3333_4444_5555_6666_7777_8888));
    assert_eq!((hr, raw.is_null()), (E_NOINTERFACE, true), "an interface the factory does not have");

    let hr =
        unsafe { super::dll::DllGetClassObject(&guid(CLSID_TEXT_SERVICE), &IClassFactory::IID, std::ptr::null_mut()) };
    assert_eq!(hr, E_POINTER, "no out pointer");
    let mut out: *mut c_void = 1 as *mut c_void;
    let hr = unsafe { super::dll::DllGetClassObject(std::ptr::null(), &IClassFactory::IID, &mut out) };
    assert_eq!((hr, out.is_null()), (E_POINTER, true), "null CLSID, and the out pointer is cleared");

    assert_eq!(super::dll::DllCanUnloadNow(), S_FALSE, "the DLL must stay mapped");
}

// ---------------------------------------------------------------------------
// The key path against the fake TSF host
// ---------------------------------------------------------------------------

struct Harness {
    service: ITfTextInputProcessorEx,
    thread_mgr: ITfThreadMgr,
    sink: ITfKeyEventSink,
    context: ITfContext,
    doc: DocRef,
    core: Rc<Core>,
    log: LogRedirect,
}

const VK_BACK: usize = 0x08;
const VK_RETURN: usize = 0x0D;
const VK_ESCAPE: usize = 0x1B;
const VK_SPACE: usize = 0x20;
const VK_DOWN: usize = 0x28;
const VK_F1: usize = 0x70;

impl Harness {
    fn new(tag: &str) -> Harness {
        let log = LogRedirect::new(tag);
        let doc = Doc::new_ref();
        let context = new_context(&doc);
        let thread_mgr = new_thread_mgr();
        let service: ITfTextInputProcessorEx = TextService::headless().into();
        let core = unsafe { AsImpl::<TextService>::as_impl(&service) }.core.clone();
        unsafe { service.ActivateEx(&thread_mgr, 7, 0) }.expect("ActivateEx");
        let sink = fake_thread_mgr(&thread_mgr).sink().expect("the key sink was advised");
        Harness { service, thread_mgr, sink, context, doc, core, log }
    }

    /// One key the way TSF delivers it: ask first, then apply if it was wanted.
    fn press(&self, vk: usize) -> bool {
        let wanted = unsafe { self.sink.OnTestKeyDown(&self.context, WPARAM(vk), LPARAM(0)) }.unwrap().as_bool();
        if !wanted {
            return false;
        }
        let eaten = unsafe { self.sink.OnKeyDown(&self.context, WPARAM(vk), LPARAM(0)) }.unwrap().as_bool();
        assert!(eaten, "a key the test phase wanted must be eaten by OnKeyDown (vk=0x{vk:x})");
        true
    }

    fn type_letters(&self, letters: &str) {
        for letter in letters.chars() {
            assert!(self.press(letter.to_ascii_uppercase() as usize), "letter {letter} must be consumed");
        }
    }

    fn text(&self) -> String {
        self.doc.borrow().string()
    }
    fn selection(&self) -> (usize, usize) {
        self.doc.borrow().selection
    }
    fn has_composition(&self) -> bool {
        self.core.composition.borrow().is_some()
    }
    fn ctx(&self) -> &FakeContext {
        fake_context(&self.context)
    }
}

#[test]
fn activation_advises_the_key_sink_and_deactivation_retires_it() {
    let h = Harness::new("activate");
    let tm = fake_thread_mgr(&h.thread_mgr);
    assert_eq!((tm.advise_calls.get(), tm.advised_client.get()), (1, Some(7)));
    assert_eq!(h.core.client_id.get(), 7);
    assert!(h.core.sink_advised.get());

    unsafe { h.service.Deactivate() }.unwrap();
    assert_eq!((tm.unadvise_calls.get(), tm.advised_client.get()), (1, None));
    assert!(!h.core.sink_advised.get());
    assert!(h.core.thread_mgr.borrow().is_none(), "the thread manager reference is released");
    // Deactivating twice (TSF can) is harmless.
    unsafe { h.service.Deactivate() }.unwrap();
    assert_eq!(tm.unadvise_calls.get(), 1, "nothing left to unadvise");
    assert!(h.log.contents().contains("had no advised sink"), "{}", h.log.contents());
}

#[test]
fn activation_without_a_thread_manager_is_e_pointer() {
    let _log = LogRedirect::new("activate-null");
    let service: ITfTextInputProcessorEx = TextService::headless().into();
    let error = unsafe { service.Activate(None::<&ITfThreadMgr>, 1) }.unwrap_err();
    assert_eq!(error.code(), E_POINTER);
    // And Deactivate without a successful Activate must not crash.
    unsafe { service.Deactivate() }.unwrap();
}

#[test]
fn the_plain_activate_entry_point_also_advises() {
    let _log = LogRedirect::new("activate-plain");
    let thread_mgr = new_thread_mgr();
    let service: ITfTextInputProcessorEx = TextService::headless().into();
    let base: ITfTextInputProcessor = service.cast().unwrap();
    unsafe { base.Activate(&thread_mgr, 3) }.unwrap();
    assert_eq!(fake_thread_mgr(&thread_mgr).advised_client.get(), Some(3));
    unsafe { base.Deactivate() }.unwrap();
}

// The regression the Go comments tell the story of: "ni" plus a space used to
// produce "你nin" because each write inserted at the composition start instead of
// replacing what was on screen.
#[test]
fn typing_replaces_the_composition_instead_of_inserting_into_it() {
    let h = Harness::new("typing");
    for (typed, want) in [("n", "n"), ("i", "ni"), ("h", "nih"), ("a", "niha"), ("o", "nihao")] {
        h.type_letters(typed);
        assert_eq!(h.text(), want, "after typing {want:?}");
    }
    assert_eq!(h.doc.borrow().compositions_started, 1, "one composition for the whole word");
    assert!(h.has_composition());
    assert_eq!(h.core.session.written_composition(), "nihao");
}

#[test]
fn space_commits_the_first_candidate_and_leaves_the_caret_behind_it() {
    let h = Harness::new("commit");
    h.type_letters("nihao");
    assert!(h.press(VK_SPACE));
    assert_eq!(h.text(), "你好", "the composition is replaced by the candidate, not added to");
    assert_eq!(h.selection(), (2, 2), "the caret sits behind the committed text");
    assert!(!h.has_composition());
    assert_eq!(h.doc.borrow().compositions_ended, 1);
    assert_eq!(h.core.session.written_composition(), "");
    assert!(h.core.pending_caret.borrow().is_none(), "the parked caret move was consumed");
    assert!(!h.core.session.candidate_ui.snapshot().visible, "the list goes away with the commit");

    // The next word starts after the committed one.
    h.type_letters("ni");
    assert_eq!(h.text(), "你好ni");
    assert!(h.press(VK_SPACE));
    assert_eq!(h.text(), format!("你好{}", candidates_for("ni")[0].text));
    assert_eq!(h.selection(), (3, 3));
    assert_eq!(h.doc.borrow().compositions_started, 2);
}

#[test]
fn a_numeric_pick_commits_that_candidate() {
    let h = Harness::new("digit");
    h.type_letters("ni");
    assert!(h.press('3' as usize));
    assert_eq!(h.text(), candidates_for("ni")[2].text);
    assert!(!h.has_composition());
    assert_eq!(h.selection(), (1, 1));
}

#[test]
fn arrows_move_the_selection_and_enter_commits_it() {
    let h = Harness::new("arrows");
    h.type_letters("ni");
    assert!(h.press(VK_DOWN));
    assert!(h.press(VK_DOWN));
    assert_eq!(h.core.session.candidate_ui.snapshot().selected, 2);
    assert_eq!(h.text(), "ni", "navigation never touches the document");
    assert!(h.press(VK_RETURN));
    assert_eq!(h.text(), candidates_for("ni")[2].text);
}

#[test]
fn backspace_shrinks_the_composition_in_place() {
    let h = Harness::new("backspace");
    h.type_letters("nih");
    assert!(h.press(VK_BACK));
    assert_eq!(h.text(), "ni");
    assert!(h.press(VK_BACK));
    assert!(h.press(VK_BACK));
    assert_eq!(h.text(), "", "an emptied composition leaves nothing behind");
    assert!(!h.has_composition());
    // With nothing composing, Backspace belongs to the application.
    assert!(!h.press(VK_BACK));
}

#[test]
fn escape_cancels_and_removes_the_pinyin_from_the_document() {
    let h = Harness::new("escape");
    h.type_letters("nihao");
    assert!(h.press(VK_ESCAPE));
    assert_eq!(h.text(), "", "a cancel must not leave the letters behind");
    assert!(!h.has_composition());
    assert_eq!(h.core.session.written_composition(), "");
    assert!(!h.core.session.candidate_ui.snapshot().visible);
    assert!(!h.press(VK_ESCAPE), "Escape with nothing composing is the application's");
}

#[test]
fn text_already_in_the_document_is_left_alone() {
    let h = Harness::new("existing");
    // The document already holds "ab" and the caret is at the end.
    h.doc.borrow_mut().text = "ab".encode_utf16().collect();
    h.doc.borrow_mut().selection = (2, 2);
    h.type_letters("ni");
    assert_eq!(h.text(), "abni");
    assert!(h.press(VK_ESCAPE));
    assert_eq!(h.text(), "ab", "cancel removes only what the composition wrote");
    h.type_letters("ni");
    assert!(h.press(VK_SPACE));
    assert_eq!(h.text(), format!("ab{}", candidates_for("ni")[0].text));
}

#[test]
fn keys_without_a_mapping_are_never_eaten() {
    let h = Harness::new("unmapped");
    assert!(!h.press(VK_F1));
    h.type_letters("ni");
    assert!(!h.press(VK_F1), "F1 is the application's even while composing");
    assert_eq!(h.text(), "ni");
}

#[test]
fn the_test_phase_never_changes_state() {
    let h = Harness::new("testphase");
    let wanted = unsafe { h.sink.OnTestKeyDown(&h.context, WPARAM('N' as usize), LPARAM(0)) }.unwrap().as_bool();
    assert!(wanted);
    assert_eq!(h.core.session.ime_state().composition, "", "OnTestKeyDown must not append the letter");
    assert_eq!(h.text(), "");
    assert_eq!(h.doc.borrow().edit_sessions, 0, "and must not touch the document");
    // key-up callbacks never eat
    assert!(!unsafe { h.sink.OnTestKeyUp(&h.context, WPARAM('N' as usize), LPARAM(0)) }.unwrap().as_bool());
    assert!(!unsafe { h.sink.OnKeyUp(&h.context, WPARAM('N' as usize), LPARAM(0)) }.unwrap().as_bool());
    let preserved = unsafe { h.sink.OnPreservedKey(&h.context, &guid(CLSID_TEXT_SERVICE)) }.unwrap();
    assert!(!preserved.as_bool());
}

#[test]
fn callbacks_survive_a_missing_context() {
    // port of TestKeySinkCallbacksSurviveWithoutAContext
    let h = Harness::new("nocontext");
    let none = None::<&ITfContext>;
    for vk in ['N' as usize, 'I' as usize] {
        assert!(unsafe { h.sink.OnTestKeyDown(none, WPARAM(vk), LPARAM(0)) }.unwrap().as_bool());
        assert!(unsafe { h.sink.OnKeyDown(none, WPARAM(vk), LPARAM(0)) }.unwrap().as_bool());
    }
    // The engine advanced even though there was nothing to write into.
    assert_eq!(h.core.session.ime_state().composition, "ni");
    assert_eq!(h.text(), "");
    assert!(!unsafe { h.sink.OnKeyUp(none, WPARAM(1), LPARAM(0)) }.unwrap().as_bool());
}

#[test]
fn the_candidate_model_sees_the_caret_the_edit_session_measured() {
    let h = Harness::new("caret");
    h.type_letters("ni");
    let snapshot = h.core.session.candidate_ui.snapshot();
    assert!(snapshot.visible);
    assert_eq!(snapshot.composition, "ni");
    // the fake puts a collapsed range at the end of "ni" (2 units) at x=120
    assert_eq!(snapshot.caret, fake_text_rect(2, 2));
    assert!(snapshot.dpi >= 96 || snapshot.dpi == 0 || snapshot.dpi > 0);
    assert_ne!(snapshot.dpi, 0);
    assert_eq!(snapshot.candidates, candidates_for("ni"));
}

#[test]
fn a_reentrant_key_callback_is_left_to_the_application_instead_of_deadlocking() {
    let h = Harness::new("reentrant");
    let sink = h.sink.clone();
    let context = h.context.clone();
    let seen = Rc::new(std::cell::Cell::new(None));
    let seen_in_hook = seen.clone();
    h.ctx().set_edit_session_hook(move || {
        // the application dispatching a key back into the TIP mid edit session
        let eaten = unsafe { sink.OnTestKeyDown(&context, WPARAM('X' as usize), LPARAM(0)) }.unwrap().as_bool();
        let also = unsafe { sink.OnKeyDown(&context, WPARAM('X' as usize), LPARAM(0)) }.unwrap().as_bool();
        seen_in_hook.set(Some((eaten, also)));
    });
    assert!(h.press('N' as usize));
    assert_eq!(seen.get(), Some((false, false)), "re-entrant keys must be declined, not applied or waited on");
    assert_eq!(h.core.session.ime_state().composition, "n", "the re-entrant 'x' must not have been typed");
    assert_eq!(h.text(), "n");
    assert!(h.log.contents().contains("re-entered"), "the log must say why: {}", h.log.contents());
    // and the flag is released afterwards
    h.ctx().set_edit_session_hook(|| {});
    h.type_letters("i");
    assert_eq!(h.text(), "ni");
}

#[test]
fn focus_changes_hide_and_restore_the_list_while_the_composition_lives() {
    let h = Harness::new("focus");
    h.type_letters("ni");
    assert!(h.core.session.candidate_ui.snapshot().visible);
    unsafe { h.sink.OnSetFocus(false) }.unwrap();
    assert!(!h.core.session.candidate_ui.snapshot().visible, "another application owns the text now");
    unsafe { h.sink.OnSetFocus(true) }.unwrap();
    assert!(h.core.session.candidate_ui.snapshot().visible, "coming back restores it");

    // Once the composition is gone, regaining focus must not resurrect a list.
    assert!(h.press(VK_ESCAPE));
    unsafe { h.sink.OnSetFocus(false) }.unwrap();
    unsafe { h.sink.OnSetFocus(true) }.unwrap();
    assert!(!h.core.session.candidate_ui.snapshot().visible);
}

#[test]
fn deactivation_takes_a_live_composition_down() {
    let h = Harness::new("deactivate-live");
    h.type_letters("nihao");
    assert_eq!(h.text(), "nihao");
    unsafe { h.service.Deactivate() }.unwrap();
    assert_eq!(h.text(), "", "the half-typed pinyin must not be left in the document");
    assert!(!h.has_composition());
    assert!(h.core.composition_context.borrow().is_none(), "the context reference is released");
    assert_eq!(h.core.session.ime_state().composition, "");
    assert!(!h.core.session.candidate_ui.snapshot().visible);
}

#[test]
fn a_composition_the_application_ended_is_replaced_not_reused() {
    let h = Harness::new("terminated");
    h.type_letters("ni");
    assert_eq!(h.doc.borrow().compositions_started, 1);
    h.ctx().terminate_composition();
    assert!(h.log.contents().contains("OnCompositionTerminated"), "{}", h.log.contents());
    // The next key finds the stored composition dead and starts a new one
    // instead of writing into a range that no longer belongs to us.
    h.type_letters("h");
    assert_eq!(h.doc.borrow().compositions_started, 2, "a fresh composition was started");
    assert!(h.has_composition());
    assert!(h.log.contents().contains("stale composition discarded"), "{}", h.log.contents());
}

#[test]
fn a_refused_edit_session_drops_the_composition_and_typing_recovers() {
    let h = Harness::new("refused");
    h.ctx().refuse_edit_sessions.set(true);
    h.type_letters("n");
    assert_eq!(h.text(), "");
    assert!(!h.has_composition(), "nothing was written, so there is nothing to track");
    assert!(h.log.contents().contains("edit session rejected"), "{}", h.log.contents());
    h.ctx().refuse_edit_sessions.set(false);
    h.type_letters("i");
    // the engine kept "ni" even though the first write was lost, so the next write is "ni"
    assert_eq!(h.text(), "ni");
    assert!(h.has_composition());
}

#[test]
fn after_a_commit_the_service_holds_no_references_into_the_document() {
    let h = Harness::new("no-leak");
    h.type_letters("ni");
    assert!(h.press(VK_SPACE));
    assert!(h.core.composition.borrow().is_none());
    assert!(h.core.pending_caret.borrow().is_none());
    // the only references to the context are the harness's own and the one the
    // service keeps to end a later composition (released at Deactivate)
    unsafe { h.service.Deactivate() }.unwrap();
    assert!(h.core.composition_context.borrow().is_none());
}

// ---------------------------------------------------------------------------
// The candidate window
// ---------------------------------------------------------------------------

fn wait_until(what: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn candidates(texts: &[&str]) -> Vec<Candidate> {
    texts
        .iter()
        .enumerate()
        .map(|(index, text)| Candidate { text: (*text).into(), index, annotation: String::new() })
        .collect()
}

#[test]
fn the_candidate_window_appears_at_the_caret_without_taking_focus_and_hides_again() {
    let _log = LogRedirect::new("window");
    let presenter = WindowPresenter::new();

    // Nothing happens while the presenter is disabled.
    presenter.publish(Snapshot { visible: true, ..Snapshot::default() });
    assert!(presenter.hwnd().is_none(), "no window before the presenter is enabled");

    presenter.set_enabled(true);
    let caret = [300, 300, 300, 320];
    presenter.publish(Snapshot {
        composition: "ni".into(),
        candidates: candidates(&["你", "拟", "尼"]),
        selected: 1,
        caret,
        dpi: 96,
        visible: true,
    });
    wait_until("the window to be created and shown", Duration::from_secs(10), || {
        presenter.hwnd().is_some_and(|hwnd| unsafe { IsWindowVisible(hwnd) }.as_bool())
    });
    let hwnd = presenter.hwnd().unwrap();

    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.unwrap();
    assert!(rect.right - rect.left > 20 && rect.bottom - rect.top > 20, "a real size: {rect:?}");
    let extra = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    assert!(extra & WS_EX_NOACTIVATE.0 != 0, "the list must never take focus from the application");
    assert!(extra & WS_EX_TOPMOST.0 != 0, "and must stay above it");

    // Placement is the pure layout function applied to this monitor's work area.
    let size = (rect.right - rect.left, rect.bottom - rect.top);
    let bounds = super::presenter::bounds_near(300, 320);
    let (x, y) = crate::layout::place_window(caret, None, size, bounds);
    assert_eq!((rect.left, rect.top), (x, y), "window rect {rect:?}, bounds {bounds:?}");

    // Let at least one WM_PAINT run; the window must survive painting.
    std::thread::sleep(Duration::from_millis(200));
    assert!(unsafe { IsWindow(Some(hwnd)) }.as_bool());

    // A newer snapshot replaces the old one (selection moves, list shrinks).
    presenter.publish(Snapshot {
        composition: "ni".into(),
        candidates: candidates(&["你"]),
        selected: 0,
        caret,
        dpi: 144,
        visible: true,
    });
    wait_until("the list to be resized for 144 DPI", Duration::from_secs(10), || {
        let mut now = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut now) }.is_ok() && (now.bottom - now.top) != size.1
    });

    presenter.publish(Snapshot { dpi: 96, ..Snapshot::default() });
    wait_until("the window to hide", Duration::from_secs(10), || !unsafe { IsWindowVisible(hwnd) }.as_bool());

    // Disabling hides too, even if a visible snapshot arrives afterwards.
    presenter.set_enabled(false);
    presenter.publish(Snapshot {
        composition: "x".into(),
        candidates: candidates(&["啊"]),
        visible: true,
        ..Snapshot::default()
    });
    std::thread::sleep(Duration::from_millis(100));
    assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool(), "a disabled presenter shows nothing");

    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}
