//! The candidate window, drawn with plain Win32/GDI.
//!
//! Port of `tip/candidate_presenter.go`. The Go version used a Walk label in a
//! vendored GUI toolkit; this one paints the list itself, so the DLL carries no
//! GUI framework and no extra goroutine/thread machinery beyond one UI thread.
//!
//! Threading follows the Go design: TSF callbacks only *publish* a snapshot (a
//! copy of plain data) and return. One dedicated thread owns the window and its
//! message loop; snapshots are coalesced (newest wins) and handed over with
//! `PostMessage`, so a key callback never waits for painting.
//!
//! The window is a topmost tool window that never takes activation, so the
//! application keeps focus and keyboard input while the list is up.

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, DeleteObject, EndPaint, FillRect, FrameRect, GetDC, GetMonitorInfoW, GetSysColor,
    GetSysColorBrush, GetTextExtentPoint32W, GetTextMetricsW, InvalidateRect, MonitorFromPoint, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, TextOutW, ValidateRect, CLEARTYPE_QUALITY, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWFRAME, COLOR_WINDOWTEXT, DEFAULT_CHARSET, HFONT, HGDIOBJ, LOGFONTW,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, TEXTMETRICW, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetCursorPos, GetMessageW, GetSystemMetrics,
    LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassExW, SetWindowPos, ShowWindow, TranslateMessage,
    HWND_TOPMOST, IDC_ARROW, MA_NOACTIVATE, MSG, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, WM_APP, WM_DESTROY, WM_ERASEBKGND, WM_MOUSEACTIVATE,
    WM_PAINT, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::candidate_log;
use crate::candidate_ui::{Presenter, Snapshot};
use crate::diag::guarded;
use crate::layout::{format_lines, place_window, scale, Rect};

const CLASS_NAME: PCWSTR = w!("PortRustWindowsIMECandidateWindow");
/// "A newer snapshot is waiting in `Shared::latest`."
const WM_PRESENT: u32 = WM_APP + 1;

// Metrics at 96 DPI; everything is scaled by the snapshot's DPI.
const FONT_HEIGHT: i32 = 20;
const PAD_X: i32 = 10;
const PAD_Y: i32 = 6;
const ROW_PAD: i32 = 3;
const MAX_WIDTH: i32 = 720;
const FONT_FACE: &str = "Microsoft YaHei UI";

struct Shared {
    /// Newest snapshot not yet shown. Older ones are replaced, never queued.
    latest: Mutex<Option<Snapshot>>,
    /// The window handle, 0 until the UI thread has created it.
    hwnd: AtomicIsize,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) struct WindowPresenter {
    state: Mutex<State>,
    shared: Arc<Shared>,
}

struct State {
    enabled: bool,
    started: bool,
}

impl WindowPresenter {
    pub fn new() -> WindowPresenter {
        WindowPresenter {
            state: Mutex::new(State { enabled: false, started: false }),
            shared: Arc::new(Shared { latest: Mutex::new(None), hwnd: AtomicIsize::new(0) }),
        }
    }

    /// Replaces the pending snapshot and wakes the UI thread.
    fn post(&self, snapshot: Snapshot) {
        *lock(&self.shared.latest) = Some(snapshot);
        let hwnd = self.shared.hwnd.load(Ordering::Acquire);
        if hwnd != 0 {
            let _ = unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_PRESENT, WPARAM(0), LPARAM(0)) };
        }
    }

    /// The window handle once the UI thread has created it (tests).
    #[cfg(test)]
    pub fn hwnd(&self) -> Option<HWND> {
        match self.shared.hwnd.load(Ordering::Acquire) {
            0 => None,
            raw => Some(HWND(raw as *mut _)),
        }
    }
}

impl Presenter for WindowPresenter {
    fn set_enabled(&self, enabled: bool) {
        let started = {
            let mut state = lock(&self.state);
            state.enabled = enabled;
            state.started
        };
        candidate_log!("presenter.enabled component=tip enabled={enabled} started={started}");
        if started && !enabled {
            // Disabling takes an open list down.
            self.post(Snapshot { dpi: 96, ..Snapshot::default() });
        }
    }

    fn publish(&self, snapshot: Snapshot) {
        candidate_log!(
            "presenter.publish component=tip visible={} candidate_count={} selected_index={} dpi={}",
            snapshot.visible,
            snapshot.candidates.len(),
            snapshot.selected,
            snapshot.dpi
        );
        {
            let mut state = lock(&self.state);
            if !state.enabled {
                candidate_log!("presenter.publish.skipped component=tip reason=disabled");
                return;
            }
            if !state.started {
                // The UI thread starts on the first snapshot, not at load: most
                // processes that load the DLL never show a list.
                state.started = true;
                let shared = Arc::clone(&self.shared);
                let spawned = std::thread::Builder::new()
                    .name("port-rust-ime-candidate-window".into())
                    .spawn(move || window_thread(shared));
                if let Err(error) = spawned {
                    candidate_log!("presenter.thread.spawn_failed component=tip error={error}");
                }
            }
        }
        self.post(snapshot);
    }
}

/// What the window procedure needs to paint, kept on the UI thread.
struct PaintState {
    lines: Vec<Vec<u16>>,
    selected: usize,
    row_height: i32,
    pad_x: i32,
    pad_y: i32,
    font: HFONT,
    font_dpi: u32,
}

thread_local! {
    static SHARED: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
    static PAINT: RefCell<Option<PaintState>> = const { RefCell::new(None) };
}

fn module_instance() -> HINSTANCE {
    super::this_module().map(|module| HINSTANCE(module.0)).unwrap_or_default()
}

fn register_window_class(instance: HINSTANCE) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&class) } == 0 {
            candidate_log!("presenter.class.register_failed component=tip");
        }
    });
}

fn window_thread(shared: Arc<Shared>) {
    let _ = guarded("CandidateWindow.thread", || {
        let instance = module_instance();
        register_window_class(instance);
        let created = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                CLASS_NAME,
                w!("Candidates"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                Some(instance),
                None,
            )
        };
        let hwnd = match created {
            Ok(hwnd) => hwnd,
            Err(error) => {
                candidate_log!("presenter.window.create_failed component=tip error={error}");
                return;
            }
        };
        SHARED.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&shared)));
        shared.hwnd.store(hwnd.0 as isize, Ordering::Release);
        candidate_log!("presenter.window.created component=tip");
        // A snapshot may have been published before the window existed.
        present_latest(hwnd);

        let mut message = MSG::default();
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        candidate_log!("presenter.window.closed component=tip");
    });
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A panic must not unwind out of a window procedure: it would abort the host.
    match message {
        WM_PRESENT => {
            let _ = guarded("CandidateWindow.present", || present_latest(hwnd));
            LRESULT(0)
        }
        WM_PAINT => {
            if guarded("CandidateWindow.paint", || paint(hwnd)).is_err() {
                // BeginPaint/EndPaint may not have run; validating stops a repaint storm.
                let _ = unsafe { ValidateRect(Some(hwnd), None) };
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        // The list must never steal focus from the application being typed into.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn present_latest(hwnd: HWND) {
    let shared = SHARED.with(|slot| slot.borrow().clone());
    let Some(shared) = shared else { return };
    let snapshot = lock(&shared.latest).take();
    if let Some(snapshot) = snapshot {
        apply(hwnd, snapshot);
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn make_font(dpi: u32) -> HFONT {
    let mut face = [0u16; 32];
    for (slot, unit) in face.iter_mut().zip(FONT_FACE.encode_utf16().take(31)) {
        *slot = unit;
    }
    let description = LOGFONTW {
        lfHeight: -scale(FONT_HEIGHT, dpi),
        lfWeight: 400,
        lfCharSet: DEFAULT_CHARSET,
        lfQuality: CLEARTYPE_QUALITY,
        lfFaceName: face,
        ..Default::default()
    };
    unsafe { CreateFontIndirectW(&description) }
}

/// The bounds a list at `(x, y)` may occupy: the work area of the nearest monitor,
/// or the virtual screen when the monitor cannot be queried.
pub(crate) fn bounds_near(x: i32, y: i32) -> Rect {
    let monitor = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        let work = info.rcWork;
        return Rect { left: work.left, top: work.top, right: work.right, bottom: work.bottom };
    }
    let (left, top) = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN)) };
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
    Rect { left, top, right: left + width.max(1), bottom: top + height.max(1) }
}

fn apply(hwnd: HWND, snapshot: Snapshot) {
    if !snapshot.visible || snapshot.candidates.is_empty() {
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        candidate_log!("presenter.window.hidden component=tip");
        return;
    }
    let dpi = if snapshot.dpi == 0 { 96 } else { snapshot.dpi };
    let lines: Vec<Vec<u16>> = format_lines(&snapshot.candidates, snapshot.selected).iter().map(|l| wide(l)).collect();

    // Measure with the font the paint will use.
    let (pad_x, pad_y, row_pad) = (scale(PAD_X, dpi), scale(PAD_Y, dpi), scale(ROW_PAD, dpi));
    let font = PAINT.with(|paint| match paint.borrow_mut().take() {
        Some(old) if old.font_dpi == dpi => old.font,
        Some(old) => {
            let _ = unsafe { DeleteObject(HGDIOBJ(old.font.0)) };
            make_font(dpi)
        }
        None => make_font(dpi),
    });
    let dc = unsafe { GetDC(Some(hwnd)) };
    let previous = unsafe { SelectObject(dc, HGDIOBJ(font.0)) };
    let mut metrics = TEXTMETRICW::default();
    let _ = unsafe { GetTextMetricsW(dc, &mut metrics) };
    let row_height = metrics.tmHeight + 2 * row_pad;
    let mut widest = 0;
    for line in &lines {
        let mut size = SIZE::default();
        if unsafe { GetTextExtentPoint32W(dc, line, &mut size) }.as_bool() {
            widest = widest.max(size.cx);
        }
    }
    unsafe {
        SelectObject(dc, previous);
        ReleaseDC(Some(hwnd), dc);
    }
    let width = (widest + 2 * pad_x).min(scale(MAX_WIDTH, dpi));
    let height = row_height * lines.len() as i32 + 2 * pad_y;

    let cursor = {
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point) }.ok().map(|()| (point.x, point.y))
    };
    let (anchor_x, anchor_y, _) = crate::layout::anchor(snapshot.caret, cursor);
    let bounds = bounds_near(anchor_x, anchor_y);
    let (x, y) = place_window(snapshot.caret, cursor, (width, height), bounds);

    PAINT.with(|paint| {
        *paint.borrow_mut() =
            Some(PaintState { lines, selected: snapshot.selected, row_height, pad_x, pad_y, font, font_dpi: dpi });
    });
    let placed =
        unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW) };
    let _ = unsafe { InvalidateRect(Some(hwnd), None, true) };
    candidate_log!(
        "presenter.window.shown component=tip candidate_count={} selected_index={} dpi={dpi} x={x} y={y} w={width} h={height} placed={}",
        snapshot.candidates.len(),
        snapshot.selected,
        placed.is_ok()
    );
}

fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let dc = unsafe { BeginPaint(hwnd, &mut ps) };
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    unsafe {
        FillRect(dc, &client, GetSysColorBrush(COLOR_WINDOW));
        FrameRect(dc, &client, GetSysColorBrush(COLOR_WINDOWFRAME));
        SetBkMode(dc, TRANSPARENT);
    }
    PAINT.with(|paint| {
        let paint = paint.borrow();
        let Some(state) = paint.as_ref() else { return };
        let previous = unsafe { SelectObject(dc, HGDIOBJ(state.font.0)) };
        for (index, line) in state.lines.iter().enumerate() {
            let top = state.pad_y + index as i32 * state.row_height;
            let row = RECT { left: 1, top, right: client.right - 1, bottom: top + state.row_height };
            let selected = index == state.selected;
            unsafe {
                if selected {
                    FillRect(dc, &row, GetSysColorBrush(COLOR_HIGHLIGHT));
                }
                let color = if selected { COLOR_HIGHLIGHTTEXT } else { COLOR_WINDOWTEXT };
                SetTextColor(dc, COLORREF(GetSysColor(color)));
                let _ = TextOutW(dc, state.pad_x, top + (state.row_height - metrics_height(dc)) / 2, line);
            }
        }
        unsafe { SelectObject(dc, previous) };
    });
    unsafe {
        let _ = EndPaint(hwnd, &ps);
    }
}

fn metrics_height(dc: windows::Win32::Graphics::Gdi::HDC) -> i32 {
    let mut metrics = TEXTMETRICW::default();
    let _ = unsafe { GetTextMetricsW(dc, &mut metrics) };
    metrics.tmHeight
}
