//! Writing the engine's answer into the document through TSF edit sessions.
//!
//! Port of `tip/composition.go`. The Go version went through a layer of C thunks
//! and released every COM pointer by hand; here the interfaces are called
//! directly and a reference is released when its wrapper is dropped, which
//! removes that whole class of leak and double-release.
//!
//! The logic is a faithful port, including the hard-won details around the
//! composition range (see [`cover_composition_range`]).

use std::cell::Cell;
use std::mem::ManuallyDrop;
use std::rc::{Rc, Weak};

use windows::core::{implement, Interface, Ref, Result, BOOL, HRESULT};
use windows::Win32::Foundation::{E_FAIL, RECT, S_OK};
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition, ITfEditSession,
    ITfEditSession_Impl, ITfRange, ITfRangeACP, TfGravity, TF_AE_NONE, TF_ANCHOR_END, TF_ANCHOR_START,
    TF_DEFAULT_SELECTION, TF_ES_READWRITE, TF_ES_SYNC, TF_SELECTION, TF_SELECTIONSTYLE,
};

use super::service::{foreground_dpi, Core};
use crate::diag::{guarded, text_for_log};
use crate::session::{utf16_nul_terminated, utf16_units, EditAction, Reply};
use crate::tip_log;

/// Result of one edit session, read back by the caller once
/// `RequestEditSession` has returned (the session is synchronous).
#[derive(Clone, Copy, Default)]
struct Outcome {
    caret: [i32; 4],
    failed: bool,
}

enum Job {
    /// Write `text` into the composition; `commit` also ends it.
    Write { text: String, commit: bool },
    /// Move the document selection onto a parked range.
    Caret(ITfRange),
}

/// One edit request. A fresh COM object is created per request, so the data
/// lives with the object that `DoEditSession` receives and never has to be
/// shared between callbacks.
#[implement(ITfEditSession)]
struct EditSession {
    core: Rc<Core>,
    context: ITfContext,
    job: Job,
    outcome: Rc<Cell<Outcome>>,
}

impl ITfEditSession_Impl for EditSession_Impl {
    /// Called by TSF with a write cookie, on the thread that asked for the
    /// session: the one that was inside `OnKeyDown`.
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let applied = guarded("ITfEditSession.DoEditSession", || self.apply(ec)).unwrap_or(false);
        if applied {
            Ok(())
        } else {
            let mut outcome = self.outcome.get();
            outcome.failed = true;
            self.outcome.set(outcome);
            Err(E_FAIL.into())
        }
    }
}

/// Hears about a composition the application ended itself. It only records the
/// fact: taking a lock here could deadlock against a key callback that is waiting
/// for the application to accept its edit session.
#[implement(ITfCompositionSink)]
struct CompositionSink {
    core: Weak<Core>,
}

impl ITfCompositionSink_Impl for CompositionSink_Impl {
    fn OnCompositionTerminated(&self, _ecwrite: u32, pcomposition: Ref<ITfComposition>) -> Result<()> {
        super::run_hr("ITfCompositionSink.OnCompositionTerminated", || {
            if let Some(core) = self.core.upgrade() {
                core.session.mark_terminated();
                tip_log!("TSF OnCompositionTerminated composition={:?}", pcomposition.as_ref().map(|c| c.as_raw()));
            }
            Ok(())
        })
    }
}

fn hr_text(error: &windows::core::Error) -> String {
    format!("0x{:08x}", error.code().0 as u32)
}

/// Places the range over exactly `units` UTF-16 code units starting at its own
/// start anchor.
///
/// A composition range does not grow with the text `SetText` writes into it: the
/// anchors stay where the composition began, so the range keeps covering nothing
/// while the document grows underneath. `SetText` on a collapsed range is an
/// insertion, so every keystroke then inserts at the composition start instead of
/// replacing the last one. That is what turned "ni" plus a space into "你nin":
/// "n" was replaced by "ni", "ni" was inserted before it as "n" + "ni" = "nin",
/// and the commit was inserted before that as "你" + "nin". Placing the range over
/// the text first makes each write a replacement of exactly what is on screen,
/// and leaves the range covering what was just written so the caret and the next
/// keystroke agree with it.
fn cover_composition_range(ec: u32, range: &ITfRange, units: usize, when: &str) -> bool {
    if unsafe { range.Collapse(ec, TF_ANCHOR_START) }.is_err() {
        tip_log!("StrategyTip {when}: collapsing the composition range failed");
        return false;
    }
    if units == 0 {
        return true;
    }
    let mut shifted = 0i32;
    // No halt condition: the plain shift is all the composition needs, and the
    // count still reports how far the anchor really went, which is how a document
    // shorter than the text we wrote shows up.
    if unsafe { range.ShiftEnd(ec, units as i32, &mut shifted, std::ptr::null()) }.is_err() {
        tip_log!("StrategyTip {when}: extending the composition range by {units} failed");
        return false;
    }
    if shifted as usize != units {
        // Short of the request means the document holds less text after the
        // composition start than the last write left there. Writing anyway would
        // overwrite text this service does not own.
        tip_log!("StrategyTip {when}: composition range covers {shifted} of {units} units");
        return false;
    }
    true
}

/// Reads the range back and reports when it does not hold exactly `want`. The
/// buffer is one unit larger than the expected text, so a range that covers too
/// much or too little cannot look like a match.
fn check_composition_text(ec: u32, range: &ITfRange, want: &str, when: &str) {
    let Some(units) = utf16_units(want) else { return };
    let mut buffer = vec![0u16; units + 1];
    let mut count = 0u32;
    if unsafe { range.GetText(ec, 0, &mut buffer, &mut count) }.is_err() {
        return;
    }
    let got = String::from_utf16_lossy(&buffer[..(count as usize).min(buffer.len())]);
    if got != want {
        tip_log!(
            "StrategyTip {when}: composition range holds {} ({count} units), expected {} ({units} units)",
            text_for_log(&got),
            text_for_log(want)
        );
    }
}

/// Records where the range sits and which way its anchors face, so a write that
/// lands somewhere unexpected is visible in the log.
fn log_composition_range(range: &ITfRange, units: usize, when: &str) {
    let (mut gravity_start, mut gravity_end) = (TfGravity(0), TfGravity(0));
    let _ = unsafe { range.GetGravity(&mut gravity_start, &mut gravity_end) };
    // ITfRange has no extent accessor of its own; the ACP flavour does. A context
    // that is not ACP based simply has no extent to report.
    let extent = range.cast::<ITfRangeACP>().and_then(|acp| {
        let (mut anchor, mut length) = (0i32, 0i32);
        unsafe { acp.GetExtent(&mut anchor, &mut length) }.map(|()| (anchor, length))
    });
    match extent {
        Ok((anchor, length)) => tip_log!(
            "StrategyTip {when} composition units={units} extent={anchor},{length} gravity={},{}",
            gravity_start.0,
            gravity_end.0
        ),
        Err(error) => tip_log!(
            "StrategyTip {when} composition units={units} gravity={},{} extent unavailable hr={}",
            gravity_start.0,
            gravity_end.0,
            hr_text(&error)
        ),
    }
}

/// Whether a stored composition still answers: an application may end it behind
/// our back, and a dead `ITfComposition` must not poison every later edit session.
fn composition_is_alive(composition: &ITfComposition) -> bool {
    unsafe { composition.GetRange() }.is_ok()
}

impl EditSession_Impl {
    /// The composition work, with a write cookie in hand.
    fn apply(&self, ec: u32) -> bool {
        let core = &self.core;
        match &self.job {
            Job::Caret(range) => self.apply_caret(ec, range),
            Job::Write { text, commit } => self.apply_write(ec, text, *commit, core),
        }
    }

    /// Collapses the document selection onto a range. Without this the caret
    /// stays where the composition began, in front of the text the IME just wrote.
    fn apply_caret(&self, ec: u32, range: &ITfRange) -> bool {
        let selection = TF_SELECTION {
            range: ManuallyDrop::new(Some(range.clone())),
            style: TF_SELECTIONSTYLE { ase: TF_AE_NONE, fInterimChar: BOOL(0) },
        };
        let result = unsafe { self.context.SetSelection(ec, std::slice::from_ref(&selection)) };
        // SetSelection only reads the struct, so the clone made above is ours to drop.
        drop(ManuallyDrop::into_inner(selection.range));
        match result {
            Ok(()) => true,
            Err(error) => {
                tip_log!("StrategyTip placing the caret: SetSelection hr={}", hr_text(&error));
                false
            }
        }
    }

    fn apply_write(&self, ec: u32, text: &str, commit: bool, core: &Rc<Core>) -> bool {
        tip_log!(
            "StrategyTip editSession apply begin ec=0x{ec:x} text={} commit={commit} previousWritten={} composition={}",
            text_for_log(text),
            text_for_log(&core.session.written_composition()),
            core.composition.borrow().is_some()
        );
        let context_composition: ITfContextComposition = match self.context.cast() {
            Ok(c) => c,
            Err(error) => {
                tip_log!("StrategyTip QueryInterface(ITfContextComposition) hr={}", hr_text(&error));
                return false;
            }
        };

        if text.is_empty() {
            // No composition and no commit: take the composition off the screen.
            tip_log!(
                "StrategyTip editSession clearing composition previousWritten={}",
                text_for_log(&core.session.written_composition())
            );
            core.end_composition_in(ec);
            tip_log!(
                "StrategyTip editSession cleared composition composition={} writtenNow={}",
                core.composition.borrow().is_some(),
                text_for_log(&core.session.written_composition())
            );
            return true;
        }

        let mut composition = core.composition.borrow().clone();
        if let Some(existing) = &composition {
            if !composition_is_alive(existing) {
                // An application may end the composition behind our back.
                tip_log!("StrategyTip stale composition discarded composition={:?}", existing.as_raw());
                core.composition.replace(None);
                composition = None;
            }
        }

        let composition = match composition {
            Some(composition) => composition,
            None => {
                let Some(selection) = self.selection_range(ec) else {
                    tip_log!("StrategyTip GetSelection failed or returned no range");
                    return false;
                };
                let sink: ITfCompositionSink = CompositionSink { core: Rc::downgrade(core) }.into();
                match unsafe { context_composition.StartComposition(ec, &selection, &sink) } {
                    Ok(created) => {
                        *core.composition.borrow_mut() = Some(created.clone());
                        core.session.store_written_composition("");
                        tip_log!("StrategyTip StartComposition ok composition={:?}", created.as_raw());
                        created
                    }
                    Err(error) => {
                        tip_log!("StrategyTip StartComposition hr={}", hr_text(&error));
                        return false;
                    }
                }
            }
        };

        let range = match unsafe { composition.GetRange() } {
            Ok(range) => range,
            Err(error) => {
                tip_log!("StrategyTip composition GetRange hr={}", hr_text(&error));
                return false;
            }
        };
        // Put the range over the text the previous write left behind before it is
        // replaced. The very first write of a composition has nothing to replace,
        // and collapsing there would throw away the selection the user asked to
        // overwrite, so that case writes into the range untouched.
        let previous = core.session.written_composition();
        if !previous.is_empty() {
            if let Some(units) = utf16_units(&previous) {
                log_composition_range(&range, units, "before SetText");
                if cover_composition_range(ec, &range, units, "before SetText") {
                    check_composition_text(ec, &range, &previous, "before SetText");
                }
            }
        }
        // Counted length plus a NUL after it: TSF reads wide strings up to a NUL, so
        // the buffer must have one even though the count is passed (see
        // `utf16_nul_terminated`).
        let Some(wide) = utf16_nul_terminated(text) else {
            tip_log!("StrategyTip composition text cannot be encoded for TSF");
            return false;
        };
        let units = wide.len() - 1;
        if let Err(error) = unsafe { range.SetText(ec, 0, &wide[..units]) } {
            tip_log!("StrategyTip range SetText hr={}", hr_text(&error));
            return false;
        }
        // Leave the range covering what was just written: the caret, the candidate
        // window and the next keystroke all read the composition from this range.
        cover_composition_range(ec, &range, units, "after SetText");
        check_composition_text(ec, &range, text, "after SetText");
        log_composition_range(&range, units, "after SetText");
        core.session.store_written_composition(text);
        self.read_caret(ec, &range);

        if commit {
            // Prepare the caret move now, but do not run SetSelection here: the
            // application could re-enter the TIP. The key sink finishes the move
            // once this callback is done (see `Core::place_pending_caret`).
            self.place_selection_at_range_end(ec, &range);
            if unsafe { composition.EndComposition(ec) }.is_err() {
                tip_log!("StrategyTip EndComposition failed");
            }
            core.composition.replace(None);
            core.session.clear_written();
        }
        true
    }

    /// The selection's range: where a new composition starts.
    fn selection_range(&self, ec: u32) -> Option<ITfRange> {
        let mut selection = [TF_SELECTION {
            range: ManuallyDrop::new(None),
            style: TF_SELECTIONSTYLE { ase: TF_AE_NONE, fInterimChar: BOOL(0) },
        }];
        let mut fetched = 0u32;
        let result = unsafe { self.context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selection, &mut fetched) };
        // TSF hands out one reference per fetched range; taking it out of the
        // ManuallyDrop makes it ours to release.
        let [selection] = selection;
        let range = ManuallyDrop::into_inner(selection.range);
        if result.is_err() || fetched != 1 {
            return None;
        }
        range
    }

    /// Prepares the selection change that puts the caret behind committed text,
    /// without running it. `SetText` and `EndComposition` leave the selection where
    /// the composition began, so the caret would show up in front of the text the
    /// user just committed. TSF never moves the selection for us: the only range
    /// the composition tracks is its own.
    ///
    /// The `SetSelection` itself is deferred because it notifies the application,
    /// and a Notepad-class host dispatches that synchronously back into a key
    /// callback. So this clones the range, collapses the clone at the end of the
    /// text and parks it on the service; the key sink runs the move afterwards.
    fn place_selection_at_range_end(&self, ec: u32, range: &ITfRange) -> bool {
        let end = match unsafe { range.Clone() } {
            Ok(end) => end,
            Err(_) => {
                tip_log!("StrategyTip placing the caret: cloning the composition range failed");
                return false;
            }
        };
        if unsafe { end.Collapse(ec, TF_ANCHOR_END) }.is_err() {
            tip_log!("StrategyTip placing the caret: collapsing the clone failed");
            return false;
        }
        self.core.drop_pending_caret();
        *self.core.pending_caret.borrow_mut() = Some(end);
        true
    }

    /// Measures the insertion point after the composition text changed.
    /// Applications that have not laid the text out yet fail on the collapsed
    /// range, so the whole composition rectangle is the fallback anchor, exactly
    /// like the C++ `CaretProbeSession`.
    fn read_caret(&self, ec: u32, composition_range: &ITfRange) {
        let Ok(view) = (unsafe { self.context.GetActiveView() }) else { return };
        let mut caret = [0i32; 4];
        let extent = |range: &ITfRange| -> Option<[i32; 4]> {
            let mut rect = RECT::default();
            let mut clipped = BOOL(0);
            unsafe { view.GetTextExt(ec, range, &mut rect, &mut clipped) }
                .ok()
                .map(|()| [rect.left, rect.top, rect.right, rect.bottom])
        };
        if let Ok(end) = unsafe { composition_range.Clone() } {
            let _ = unsafe { end.Collapse(ec, TF_ANCHOR_END) };
            if let Some(rect) = extent(&end) {
                caret = rect;
            }
        }
        if caret[2] <= caret[0] && caret[3] <= caret[1] {
            if let Some(rect) = extent(composition_range) {
                caret = rect;
            }
        }
        let mut outcome = self.outcome.get();
        outcome.caret = caret;
        self.outcome.set(outcome);
    }
}

impl Core {
    /// Asks the context for a synchronous read-write session. `RequestEditSession`
    /// returns only after `DoEditSession` has finished, so the outcome is ready.
    /// Returns whether the session ran and succeeded, and what it measured.
    fn request_edit_session(self: &Rc<Self>, context: &ITfContext, job: Job) -> (bool, Outcome, Option<String>) {
        let outcome = Rc::new(Cell::new(Outcome::default()));
        let session: ITfEditSession =
            EditSession { core: self.clone(), context: context.clone(), job, outcome: outcome.clone() }.into();
        let requested =
            unsafe { context.RequestEditSession(self.client_id.get(), &session, TF_ES_SYNC | TF_ES_READWRITE) };
        let outcome = outcome.get();
        let (session_hr, request_hr): (HRESULT, HRESULT) = match &requested {
            Ok(session_hr) => (*session_hr, S_OK),
            Err(error) => (S_OK, error.code()),
        };
        let ok = requested.is_ok() && session_hr.is_ok() && !outcome.failed;
        let detail = (!ok).then(|| {
            format!("hr=0x{:08x} session=0x{:08x} failed={}", request_hr.0 as u32, session_hr.0 as u32, outcome.failed)
        });
        (ok, outcome, detail)
    }

    /// Writes `text` into the document through an edit session and returns where
    /// the caret ended up.
    fn run_edit_session(self: &Rc<Self>, context: &ITfContext, text: &str, commit: bool) -> [i32; 4] {
        let (ok, outcome, detail) = self.request_edit_session(context, Job::Write { text: text.to_owned(), commit });
        if !ok {
            // A rejected session must not stay broken: drop the composition so the
            // next keystroke starts a fresh one, exactly like the C++ service.
            tip_log!("StrategyTip edit session rejected {}", detail.unwrap_or_default());
            self.drop_composition();
            return outcome.caret;
        }
        let caret = outcome.caret;
        tip_log!(
            "StrategyTip edit session ok commit={commit} text={} caret={},{},{},{}",
            text_for_log(text),
            caret[0],
            caret[1],
            caret[2],
            caret[3]
        );
        caret
    }

    /// Applies a parked selection using a fresh TSF write cookie:
    /// `ITfContext::SetSelection` cannot be called outside an edit session.
    fn run_caret_edit_session(self: &Rc<Self>, context: &ITfContext, caret_range: ITfRange) {
        let (ok, _, detail) = self.request_edit_session(context, Job::Caret(caret_range));
        if !ok {
            tip_log!("StrategyTip caret edit session rejected {}", detail.unwrap_or_default());
        }
    }

    /// Runs a parked selection change against `context`, the one handed to the
    /// `OnKeyDown` call that parked it. The key sink calls this after the busy flag
    /// is released, so a re-entrant key callback is handled normally. A failure
    /// only loses the caret: the text is already committed.
    pub fn place_pending_caret(self: &Rc<Self>, context: &ITfContext) {
        let parked = self.pending_caret.borrow_mut().take();
        if let Some(parked) = parked {
            self.run_caret_edit_session(context, parked);
        }
    }

    /// Releases a parked selection change that never ran.
    pub fn drop_pending_caret(&self) {
        self.pending_caret.borrow_mut().take();
    }

    /// Removes the composition from inside an edit session. Ending a composition
    /// leaves its text in the document as ordinary text, so the letters have to be
    /// cleared first: a composition that ends on a cancel, on a key the engine did
    /// not take, or on a reply with no text must not leave the pinyin behind. A
    /// commit does the opposite and replaces the text itself.
    pub fn end_composition_in(&self, ec: u32) {
        let Some(composition) = self.composition.borrow().clone() else { return };
        if let Ok(range) = unsafe { composition.GetRange() } {
            // Place the range over the text that has to go first. Clearing a
            // collapsed range deletes nothing, which is how the pinyin survived a
            // cancel that was meant to erase it.
            let previous = self.session.written_composition();
            if !previous.is_empty() {
                if let Some(units) = utf16_units(&previous) {
                    log_composition_range(&range, units, "before clearing");
                    if cover_composition_range(ec, &range, units, "before clearing") {
                        check_composition_text(ec, &range, &previous, "before clearing");
                    }
                }
            }
            // An empty slice would carry a dangling pointer; this one points at a NUL.
            let nul = [0u16];
            if unsafe { range.SetText(ec, 0, &nul[..0]) }.is_err() {
                tip_log!("StrategyTip clearing the composition text failed");
            }
        }
        if unsafe { composition.EndComposition(ec) }.is_err() {
            tip_log!("StrategyTip EndComposition on empty text failed");
        }
        self.composition.replace(None);
        self.session.clear_written();
    }

    /// Takes the composition down through its own edit session. The context must
    /// be one the caller was handed (or one we hold a reference to).
    pub fn end_composition(self: &Rc<Self>, context: Option<&ITfContext>) {
        if self.composition.borrow().is_none() {
            return;
        }
        match context {
            // Without a context there is no edit session, so the reference is
            // dropped and the next keystroke starts a new composition.
            None => self.drop_composition(),
            Some(context) => {
                self.run_edit_session(context, "", false);
            }
        }
    }

    /// Forgets the composition and what was written into it.
    pub fn drop_composition(&self) {
        self.composition.replace(None);
        // Whatever was written belongs to a composition that no longer exists, so
        // the next one starts with nothing to replace.
        self.session.clear_written();
    }

    /// Drops a composition whose key was not consumed, so the next keystroke
    /// starts from a clean slate.
    pub fn forget_composition(self: &Rc<Self>, context: &ITfContext) {
        if self.composition.borrow().is_some() {
            self.end_composition(Some(context));
        }
    }

    /// Writes one key reply into the document and reports where the caret ended
    /// up. Runs whatever the reply says: composition text while the candidate list
    /// is open, committed text when a candidate is chosen, nothing when the
    /// composition ends.
    pub fn apply_composition(self: &Rc<Self>, context: &ITfContext, reply: &Reply) -> [i32; 4] {
        tip_log!(
            "StrategyTip applyComposition begin composition={} commit={} previousWritten={} range={}",
            text_for_log(&reply.composition),
            reply.commit.as_deref().map(text_for_log).unwrap_or_else(|| "none".into()),
            text_for_log(&self.session.written_composition()),
            self.composition.borrow().is_some()
        );
        *self.composition_context.borrow_mut() = Some(context.clone());
        if self.session.take_terminated_for(&reply.composition) {
            // The application ended the composition since the last key: the
            // handle is stale and the edit session will start a new one.
            self.drop_composition();
        }
        match reply.edit_action() {
            EditAction::EndComposition => {
                // Nothing to draw and nothing to commit: end what is on screen.
                tip_log!(
                    "StrategyTip applyReply ending empty composition previousWritten={} range={}",
                    text_for_log(&self.session.written_composition()),
                    self.composition.borrow().is_some()
                );
                self.end_composition(Some(context));
                [0; 4]
            }
            EditAction::Write { text, commit } => self.run_edit_session(context, &text, commit),
        }
    }
}

/// DPI of the foreground window, re-exported for the key path.
#[allow(dead_code)]
pub(crate) fn current_dpi() -> u32 {
    foreground_dpi()
}
