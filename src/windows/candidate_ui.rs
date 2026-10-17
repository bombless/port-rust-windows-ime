//! Minimal native candidate popup used by the first TSF UI iteration.
//!
//! The window is non-activating and receives only an owned engine snapshot;
//! no TSF COM pointers cross into the presentation helper.
use std::sync::{Mutex, OnceLock};

use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{HWND, POINT},
        Graphics::Gdi::ClientToScreen,
        UI::WindowsAndMessaging::{
            CreateWindowExW, GetCursorPos, GetGUIThreadInfo, SetWindowPos, SetWindowTextW,
            ShowWindow, GUITHREADINFO, HWND_TOPMOST, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE,
            WS_BORDER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
};

use crate::engine::Snapshot;

static WINDOW: OnceLock<Mutex<Option<isize>>> = OnceLock::new();

// Store the native handle as an integer so the global mutex is `Send`/`Sync`.
// HWND itself wraps a raw pointer and therefore cannot be stored in a static Mutex.
fn window_slot() -> &'static Mutex<Option<isize>> {
    WINDOW.get_or_init(|| Mutex::new(None))
}

/// Hide the popup, for example when the TSF service is deactivated.
pub fn hide() {
    if let Ok(slot) = window_slot().lock() {
        if let Some(hwnd) = *slot {
            unsafe {
                let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
            }
        }
    }
}

/// Find the focused GUI thread's caret in screen coordinates. This mirrors the
/// Go presenter's preference for a text caret over the mouse cursor, without
/// retaining TSF context/range COM pointers in the UI helper.
fn caret_anchor() -> Option<(i32, i32)> {
    unsafe {
        let mut info: GUITHREADINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        if GetGUIThreadInfo(0, &mut info).is_err() || info.hwndCaret.0.is_null() {
            return None;
        }
        // The caret rectangle is client-relative. Place the popup below its
        // lower-left corner, converted to virtual-screen coordinates.
        let mut point = POINT {
            x: info.rcCaret.left,
            y: info.rcCaret.bottom,
        };
        if ClientToScreen(info.hwndCaret, &mut point).is_err() {
            return None;
        }
        Some((point.x, point.y))
    }
}

/// Publish the current candidate snapshot. The popup is hidden when no
/// composition/candidates are available.
pub fn update(snapshot: &Snapshot) {
    let Ok(mut slot) = window_slot().lock() else {
        return;
    };

    if !snapshot.visible || snapshot.candidates.is_empty() {
        if let Some(hwnd) = *slot {
            unsafe {
                let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
            }
        }
        return;
    }

    let candidates: Vec<String> = snapshot
        .candidates
        .iter()
        .take(9)
        .enumerate()
        .map(|(index, candidate)| {
            let marker = if index == snapshot.selected {
                "▶"
            } else {
                " "
            };
            format!("{marker} {}. {}", index + 1, candidate.text)
        })
        .collect();
    let text = format!("{}\r\n{}", snapshot.composition, candidates.join("\r\n"));
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();

    // Keep the popup compact while allowing candidates to be read vertically
    // instead of clipping a single long horizontal row.
    let longest_line = text
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let width = (longest_line as i32 * 16 + 28).clamp(220, 520);
    let candidate_count = candidates.len() as i32;
    let height = (candidate_count * 24 + 36).clamp(60, 260);

    let hwnd = if let Some(hwnd) = *slot {
        HWND(hwnd as *mut _)
    } else {
        let created = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                PCWSTR(wide.as_ptr()),
                WS_POPUP | WS_BORDER,
                80,
                80,
                width,
                height,
                None,
                None,
                None,
                None,
            )
        };
        let Ok(hwnd) = created else {
            return;
        };
        *slot = Some(hwnd.0 as isize);
        hwnd
    };

    // Prefer the focused text caret. Cursor anchoring is only a fallback for
    // hosts that do not expose a GUI caret through GetGUIThreadInfo.
    let (x, y) = caret_anchor().unwrap_or_else(|| {
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point).is_ok() } {
            (point.x, point.y + 20)
        } else {
            (80, 80)
        }
    });
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}
