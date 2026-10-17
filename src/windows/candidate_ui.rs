//! Minimal native candidate popup used by the first TSF UI iteration.
//!
//! The window is non-activating and receives only an owned engine snapshot;
//! no TSF COM pointers cross into the presentation helper.
use std::sync::{Mutex, OnceLock};

use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::POINT,
        UI::{
            Input::KeyboardAndMouse::GetCursorPos,
            WindowsAndMessaging::{
                CreateWindowExW, SetWindowTextW, ShowWindow, SetWindowPos, HWND_TOPMOST,
                SWP_NOACTIVATE, WS_BORDER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
                WS_EX_TOPMOST, WS_POPUP, SS_LEFT, SW_HIDE, SW_SHOWNOACTIVATE,
            },
        },
    },
};

use crate::engine::Snapshot;

static WINDOW: OnceLock<Mutex<Option<windows::Win32::Foundation::HWND>>> = OnceLock::new();

fn window_slot() -> &'static Mutex<Option<windows::Win32::Foundation::HWND>> {
    WINDOW.get_or_init(|| Mutex::new(None))
}

/// Publish the current candidate snapshot. The popup is hidden when no
/// composition/candidates are available.
pub fn update(snapshot: &Snapshot) {
    let Ok(mut slot) = window_slot().lock() else {
        return;
    };

    if !snapshot.visible || snapshot.candidates.is_empty() {
        if let Some(hwnd) = slot.as_ref() {
            unsafe { ShowWindow(*hwnd, SW_HIDE) };
        }
        return;
    }

    let mut lines = Vec::new();
    for (index, candidate) in snapshot.candidates.iter().take(9).enumerate() {
        let marker = if index == snapshot.selected { "▶" } else { " " };
        lines.push(format!("{marker} {}. {}", index + 1, candidate.text));
    }
    let text = format!("{}\r\n{}", snapshot.composition, lines.join("    "));
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let width = (text.chars().count() as i32 * 12 + 32).clamp(220, 640);
    let height = 64;

    let hwnd = if let Some(hwnd) = slot.as_ref() {
        *hwnd
    } else {
        let created = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                PCWSTR(wide.as_ptr()),
                WS_POPUP | WS_BORDER | SS_LEFT,
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
        *slot = Some(hwnd);
        hwnd
    };

    let mut point = POINT::default();
    let (x, y) = if unsafe { GetCursorPos(&mut point).is_ok() } {
        (point.x, point.y + 20)
    } else {
        (80, 80)
    };
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE,
        );
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}
