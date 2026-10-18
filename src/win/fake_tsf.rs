//! A fake TSF host for tests: just enough of a document, ranges, compositions, a
//! context and a thread manager to drive the real `TextService` through its COM
//! interfaces on any Windows machine, with no real application or ctfmon.
//!
//! The Go port could not test the edit-session code at all ("the write into a
//! real context can only be exercised against a live TSF host"). With this, the
//! composition-range logic that once produced "你nin" runs in CI.
//!
//! The document models the behaviour the Go comments recorded from real hosts:
//! `SetText` does **not** grow a range. The anchors stay where the composition
//! began, so after a write the range is collapsed at its start. That is the
//! pessimistic case, and the one the port's `cover_composition_range` exists for.

use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::rc::Rc;

use windows::core::{implement, AsImpl, Interface, Ref, Result, BOOL, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL, HWND, LPARAM, POINT, RECT, S_OK, WPARAM};
use windows::Win32::UI::TextServices::*;

/// The write cookie the fake hands to `DoEditSession`.
pub const EC: u32 = 0x42;

pub struct Doc {
    pub text: Vec<u16>,
    /// Selection as (start, end) offsets in UTF-16 units.
    pub selection: (usize, usize),
    pub compositions_started: u32,
    pub compositions_ended: u32,
    pub edit_sessions: u32,
}

pub type DocRef = Rc<RefCell<Doc>>;

impl Doc {
    pub fn new_ref() -> DocRef {
        Rc::new(RefCell::new(Doc {
            text: Vec::new(),
            selection: (0, 0),
            compositions_started: 0,
            compositions_ended: 0,
            edit_sessions: 0,
        }))
    }
    pub fn string(&self) -> String {
        String::from_utf16_lossy(&self.text)
    }
}

fn nyi<T>() -> Result<T> {
    Err(E_NOTIMPL.into())
}

// ---------------------------------------------------------------------------
// ITfRange
// ---------------------------------------------------------------------------

#[implement(ITfRange)]
pub struct FakeRange {
    doc: DocRef,
    start: Cell<usize>,
    end: Cell<usize>,
}

pub fn new_range(doc: &DocRef, start: usize, end: usize) -> ITfRange {
    FakeRange { doc: doc.clone(), start: Cell::new(start), end: Cell::new(end) }.into()
}

/// Anchors of a range this fake created.
fn anchors(range: &ITfRange) -> (usize, usize) {
    let fake: &FakeRange = unsafe { AsImpl::<FakeRange>::as_impl(range) };
    (fake.start.get(), fake.end.get())
}

impl ITfRange_Impl for FakeRange_Impl {
    fn GetText(&self, _ec: u32, _flags: u32, pchtext: PWSTR, cchmax: u32, pcch: *mut u32) -> Result<()> {
        let doc = self.doc.borrow();
        let (start, end) = (self.start.get(), self.end.get());
        let count = (end - start).min(cchmax as usize);
        unsafe {
            std::ptr::copy_nonoverlapping(doc.text[start..start + count].as_ptr(), pchtext.0, count);
            *pcch = count as u32;
        }
        Ok(())
    }

    fn SetText(&self, _ec: u32, _flags: u32, pchtext: &PCWSTR, cch: i32) -> Result<()> {
        let new: Vec<u16> = if cch <= 0 || pchtext.is_null() {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(pchtext.0, cch as usize) }.to_vec()
        };
        let (start, end) = (self.start.get(), self.end.get());
        self.doc.borrow_mut().text.splice(start..end, new);
        // The range does not grow with what was written: it stays at its start.
        self.end.set(start);
        Ok(())
    }

    fn GetFormattedText(&self, _ec: u32) -> Result<windows::Win32::System::Com::IDataObject> {
        nyi()
    }
    fn GetEmbedded(&self, _ec: u32, _service: *const GUID, _riid: *const GUID) -> Result<windows::core::IUnknown> {
        nyi()
    }
    fn InsertEmbedded(
        &self,
        _ec: u32,
        _flags: u32,
        _data: Ref<windows::Win32::System::Com::IDataObject>,
    ) -> Result<()> {
        nyi()
    }
    fn ShiftStart(&self, _ec: u32, _cchreq: i32, _pcch: *mut i32, _halt: *const TF_HALTCOND) -> Result<()> {
        nyi()
    }

    fn ShiftEnd(&self, _ec: u32, cchreq: i32, pcch: *mut i32, _halt: *const TF_HALTCOND) -> Result<()> {
        let len = self.doc.borrow().text.len() as i64;
        let old = self.end.get() as i64;
        let new = (old + i64::from(cchreq)).clamp(self.start.get() as i64, len);
        self.end.set(new as usize);
        if !pcch.is_null() {
            unsafe { *pcch = (new - old) as i32 };
        }
        Ok(())
    }

    fn ShiftStartToRange(&self, _ec: u32, _range: Ref<ITfRange>, _apos: TfAnchor) -> Result<()> {
        nyi()
    }
    fn ShiftEndToRange(&self, _ec: u32, _range: Ref<ITfRange>, _apos: TfAnchor) -> Result<()> {
        nyi()
    }
    fn ShiftStartRegion(&self, _ec: u32, _dir: TfShiftDir) -> Result<BOOL> {
        nyi()
    }
    fn ShiftEndRegion(&self, _ec: u32, _dir: TfShiftDir) -> Result<BOOL> {
        nyi()
    }
    fn IsEmpty(&self, _ec: u32) -> Result<BOOL> {
        Ok(BOOL::from(self.start.get() == self.end.get()))
    }

    fn Collapse(&self, _ec: u32, apos: TfAnchor) -> Result<()> {
        if apos == TF_ANCHOR_START {
            self.end.set(self.start.get());
        } else {
            self.start.set(self.end.get());
        }
        Ok(())
    }

    fn IsEqualStart(&self, _ec: u32, _with: Ref<ITfRange>, _apos: TfAnchor) -> Result<BOOL> {
        nyi()
    }
    fn IsEqualEnd(&self, _ec: u32, _with: Ref<ITfRange>, _apos: TfAnchor) -> Result<BOOL> {
        nyi()
    }
    fn CompareStart(&self, _ec: u32, _with: Ref<ITfRange>, _apos: TfAnchor) -> Result<i32> {
        nyi()
    }
    fn CompareEnd(&self, _ec: u32, _with: Ref<ITfRange>, _apos: TfAnchor) -> Result<i32> {
        nyi()
    }
    fn AdjustForInsert(&self, _ec: u32, _cch: u32) -> Result<BOOL> {
        nyi()
    }

    fn GetGravity(&self, pgstart: *mut TfGravity, pgend: *mut TfGravity) -> Result<()> {
        unsafe {
            *pgstart = TF_GRAVITY_BACKWARD;
            *pgend = TF_GRAVITY_FORWARD;
        }
        Ok(())
    }
    fn SetGravity(&self, _ec: u32, _start: TfGravity, _end: TfGravity) -> Result<()> {
        nyi()
    }

    fn Clone(&self) -> Result<ITfRange> {
        Ok(new_range(&self.doc, self.start.get(), self.end.get()))
    }
    fn GetContext(&self) -> Result<ITfContext> {
        nyi()
    }
}

// ---------------------------------------------------------------------------
// ITfComposition
// ---------------------------------------------------------------------------

#[implement(ITfComposition)]
pub struct FakeComposition {
    doc: DocRef,
    range: ITfRange,
    ended: Cell<bool>,
    sink: ITfCompositionSink,
}

impl ITfComposition_Impl for FakeComposition_Impl {
    fn GetRange(&self) -> Result<ITfRange> {
        // A composition the application ended no longer answers.
        if self.ended.get() {
            Err(E_FAIL.into())
        } else {
            Ok(self.range.clone())
        }
    }
    fn ShiftStart(&self, _ec: u32, _new_start: Ref<ITfRange>) -> Result<()> {
        nyi()
    }
    fn ShiftEnd(&self, _ec: u32, _new_end: Ref<ITfRange>) -> Result<()> {
        nyi()
    }
    fn EndComposition(&self, _ec: u32) -> Result<()> {
        if !self.ended.replace(true) {
            self.doc.borrow_mut().compositions_ended += 1;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ITfContext + ITfContextComposition (one object, like a real context)
// ---------------------------------------------------------------------------

#[implement(ITfContext, ITfContextComposition)]
pub struct FakeContext {
    pub doc: DocRef,
    composition: RefCell<Option<ITfComposition>>,
    /// Runs inside `RequestEditSession`, just before the session is called: the
    /// way a test makes the "application" re-enter the key sink.
    on_edit_session: RefCell<Option<Box<dyn Fn()>>>,
    /// When set, `RequestEditSession` refuses, like a document that went away.
    pub refuse_edit_sessions: Cell<bool>,
}

pub fn new_context(doc: &DocRef) -> ITfContext {
    FakeContext {
        doc: doc.clone(),
        composition: RefCell::new(None),
        on_edit_session: RefCell::new(None),
        refuse_edit_sessions: Cell::new(false),
    }
    .into()
}

pub fn fake_context(context: &ITfContext) -> &FakeContext {
    unsafe { AsImpl::<FakeContext>::as_impl(context) }
}

impl FakeContext {
    pub fn set_edit_session_hook(&self, hook: impl Fn() + 'static) {
        *self.on_edit_session.borrow_mut() = Some(Box::new(hook));
    }

    /// Simulates the application ending the composition itself.
    pub fn terminate_composition(&self) {
        let composition = self.composition.borrow().clone();
        if let Some(composition) = composition {
            let fake: &FakeComposition = unsafe { AsImpl::<FakeComposition>::as_impl(&composition) };
            if !fake.ended.replace(true) {
                self.doc.borrow_mut().compositions_ended += 1;
            }
            let _ = unsafe { fake.sink.OnCompositionTerminated(EC, &composition) };
        }
    }
}

impl ITfContext_Impl for FakeContext_Impl {
    fn RequestEditSession(
        &self,
        _tid: u32,
        pes: Ref<ITfEditSession>,
        _flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    ) -> Result<HRESULT> {
        if self.refuse_edit_sessions.get() {
            return Err(E_FAIL.into());
        }
        let session = pes.ok()?;
        self.doc.borrow_mut().edit_sessions += 1;
        if let Some(hook) = self.on_edit_session.borrow().as_ref() {
            hook();
        }
        Ok(match unsafe { session.DoEditSession(EC) } {
            Ok(()) => S_OK,
            Err(error) => error.code(),
        })
    }

    fn InWriteSession(&self, _tid: u32) -> Result<BOOL> {
        nyi()
    }

    fn GetSelection(
        &self,
        _ec: u32,
        _index: u32,
        _count: u32,
        pselection: *mut TF_SELECTION,
        pcfetched: *mut u32,
    ) -> Result<()> {
        let (start, end) = self.doc.borrow().selection;
        unsafe {
            pselection.write(TF_SELECTION {
                range: ManuallyDrop::new(Some(new_range(&self.doc, start, end))),
                style: TF_SELECTIONSTYLE { ase: TF_AE_NONE, fInterimChar: BOOL(0) },
            });
            *pcfetched = 1;
        }
        Ok(())
    }

    fn SetSelection(&self, _ec: u32, count: u32, pselection: *const TF_SELECTION) -> Result<()> {
        if count == 0 || pselection.is_null() {
            return Ok(());
        }
        let selection = unsafe { &*pselection };
        let range = (*selection.range).as_ref().ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?;
        self.doc.borrow_mut().selection = anchors(range);
        Ok(())
    }

    fn GetStart(&self, _ec: u32) -> Result<ITfRange> {
        nyi()
    }
    fn GetEnd(&self, _ec: u32) -> Result<ITfRange> {
        nyi()
    }
    fn GetActiveView(&self) -> Result<ITfContextView> {
        Ok(FakeView { doc: self.doc.clone() }.into())
    }
    fn EnumViews(&self) -> Result<IEnumTfContextViews> {
        nyi()
    }
    fn GetStatus(&self) -> Result<TS_STATUS> {
        nyi()
    }
    fn GetProperty(&self, _guid: *const GUID) -> Result<ITfProperty> {
        nyi()
    }
    fn GetAppProperty(&self, _guid: *const GUID) -> Result<ITfReadOnlyProperty> {
        nyi()
    }
    fn TrackProperties(
        &self,
        _props: *const *const GUID,
        _count: u32,
        _app_props: *const *const GUID,
        _app_count: u32,
    ) -> Result<ITfReadOnlyProperty> {
        nyi()
    }
    fn EnumProperties(&self) -> Result<IEnumTfProperties> {
        nyi()
    }
    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        nyi()
    }
    fn CreateRangeBackup(&self, _ec: u32, _range: Ref<ITfRange>) -> Result<ITfRangeBackup> {
        nyi()
    }
}

impl ITfContextComposition_Impl for FakeContext_Impl {
    fn StartComposition(
        &self,
        _ec: u32,
        pcompositionrange: Ref<ITfRange>,
        psink: Ref<ITfCompositionSink>,
    ) -> Result<ITfComposition> {
        let (start, end) = anchors(pcompositionrange.ok()?);
        let composition: ITfComposition = FakeComposition {
            doc: self.doc.clone(),
            range: new_range(&self.doc, start, end),
            ended: Cell::new(false),
            sink: psink.ok()?.clone(),
        }
        .into();
        self.doc.borrow_mut().compositions_started += 1;
        *self.composition.borrow_mut() = Some(composition.clone());
        Ok(composition)
    }
    fn EnumCompositions(&self) -> Result<IEnumITfCompositionView> {
        nyi()
    }
    fn FindComposition(&self, _ec: u32, _range: Ref<ITfRange>) -> Result<IEnumITfCompositionView> {
        nyi()
    }
    fn TakeOwnership(
        &self,
        _ec: u32,
        _composition: Ref<ITfCompositionView>,
        _sink: Ref<ITfCompositionSink>,
    ) -> Result<ITfComposition> {
        nyi()
    }
}

// ---------------------------------------------------------------------------
// ITfContextView
// ---------------------------------------------------------------------------

/// Screen position of a range in the fake: 10 px per unit from x=100, 20 px high
/// at y=200. A collapsed range has zero width and real height, which is what real
/// hosts report for an insertion point.
pub fn fake_text_rect(start: usize, end: usize) -> [i32; 4] {
    [100 + 10 * start as i32, 200, 100 + 10 * end as i32, 220]
}

#[implement(ITfContextView)]
struct FakeView {
    #[allow(dead_code)]
    doc: DocRef,
}

impl ITfContextView_Impl for FakeView_Impl {
    fn GetRangeFromPoint(&self, _ec: u32, _pt: *const POINT, _flags: u32) -> Result<ITfRange> {
        nyi()
    }
    fn GetTextExt(&self, _ec: u32, prange: Ref<ITfRange>, prc: *mut RECT, pfclipped: *mut BOOL) -> Result<()> {
        let (start, end) = anchors(prange.ok()?);
        let [left, top, right, bottom] = fake_text_rect(start, end);
        unsafe {
            *prc = RECT { left, top, right, bottom };
            *pfclipped = BOOL(0);
        }
        Ok(())
    }
    fn GetScreenExt(&self) -> Result<RECT> {
        nyi()
    }
    fn GetWnd(&self) -> Result<HWND> {
        nyi()
    }
}

// ---------------------------------------------------------------------------
// ITfThreadMgr + ITfKeystrokeMgr
// ---------------------------------------------------------------------------

#[implement(ITfThreadMgr, ITfKeystrokeMgr)]
pub struct FakeThreadMgr {
    sink: RefCell<Option<ITfKeyEventSink>>,
    pub advised_client: Cell<Option<u32>>,
    pub advise_calls: Cell<u32>,
    pub unadvise_calls: Cell<u32>,
}

pub fn new_thread_mgr() -> ITfThreadMgr {
    FakeThreadMgr {
        sink: RefCell::new(None),
        advised_client: Cell::new(None),
        advise_calls: Cell::new(0),
        unadvise_calls: Cell::new(0),
    }
    .into()
}

pub fn fake_thread_mgr(thread_mgr: &ITfThreadMgr) -> &FakeThreadMgr {
    unsafe { AsImpl::<FakeThreadMgr>::as_impl(thread_mgr) }
}

impl FakeThreadMgr {
    /// The sink the text service advised, as TSF would hold it.
    pub fn sink(&self) -> Option<ITfKeyEventSink> {
        self.sink.borrow().clone()
    }
}

impl ITfThreadMgr_Impl for FakeThreadMgr_Impl {
    fn Activate(&self) -> Result<u32> {
        nyi()
    }
    fn Deactivate(&self) -> Result<()> {
        nyi()
    }
    fn CreateDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        nyi()
    }
    fn EnumDocumentMgrs(&self) -> Result<IEnumTfDocumentMgrs> {
        nyi()
    }
    fn GetFocus(&self) -> Result<ITfDocumentMgr> {
        nyi()
    }
    fn SetFocus(&self, _dim: Ref<ITfDocumentMgr>) -> Result<()> {
        nyi()
    }
    fn AssociateFocus(&self, _hwnd: HWND, _dim: Ref<ITfDocumentMgr>) -> Result<ITfDocumentMgr> {
        nyi()
    }
    fn IsThreadFocus(&self) -> Result<BOOL> {
        nyi()
    }
    fn GetFunctionProvider(&self, _clsid: *const GUID) -> Result<ITfFunctionProvider> {
        nyi()
    }
    fn EnumFunctionProviders(&self) -> Result<IEnumTfFunctionProviders> {
        nyi()
    }
    fn GetGlobalCompartment(&self) -> Result<ITfCompartmentMgr> {
        nyi()
    }
}

impl ITfKeystrokeMgr_Impl for FakeThreadMgr_Impl {
    fn AdviseKeyEventSink(&self, tid: u32, psink: Ref<ITfKeyEventSink>, _foreground: BOOL) -> Result<()> {
        self.advise_calls.set(self.advise_calls.get() + 1);
        self.advised_client.set(Some(tid));
        *self.sink.borrow_mut() = Some(psink.ok()?.clone());
        Ok(())
    }
    fn UnadviseKeyEventSink(&self, _tid: u32) -> Result<()> {
        self.unadvise_calls.set(self.unadvise_calls.get() + 1);
        self.advised_client.set(None);
        self.sink.borrow_mut().take();
        Ok(())
    }
    fn GetForeground(&self) -> Result<GUID> {
        nyi()
    }
    fn TestKeyDown(&self, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        nyi()
    }
    fn TestKeyUp(&self, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        nyi()
    }
    fn KeyDown(&self, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        nyi()
    }
    fn KeyUp(&self, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        nyi()
    }
    fn GetPreservedKey(&self, _pic: Ref<ITfContext>, _prekey: *const TF_PRESERVEDKEY) -> Result<GUID> {
        nyi()
    }
    fn IsPreservedKey(&self, _guid: *const GUID, _prekey: *const TF_PRESERVEDKEY) -> Result<BOOL> {
        nyi()
    }
    fn PreserveKey(
        &self,
        _tid: u32,
        _guid: *const GUID,
        _prekey: *const TF_PRESERVEDKEY,
        _desc: &PCWSTR,
        _cch: u32,
    ) -> Result<()> {
        nyi()
    }
    fn UnpreserveKey(&self, _guid: *const GUID, _prekey: *const TF_PRESERVEDKEY) -> Result<()> {
        nyi()
    }
    fn SetPreservedKeyDescription(&self, _guid: *const GUID, _desc: &PCWSTR, _cch: u32) -> Result<()> {
        nyi()
    }
    fn GetPreservedKeyDescription(&self, _guid: *const GUID) -> Result<windows::core::BSTR> {
        nyi()
    }
    fn SimulatePreservedKey(&self, _pic: Ref<ITfContext>, _guid: *const GUID) -> Result<BOOL> {
        nyi()
    }
}

/// Keeps `Interface` in scope for `cast` in tests that use this module.
#[allow(dead_code)]
fn _uses_interface<T: Interface>() {}
