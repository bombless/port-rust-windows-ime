//! Candidate-window text and placement, as pure functions.
//!
//! The Go presenter computed these inline in its window code. They are split out
//! so the geometry can be tested without a window. Three things differ from the
//! Go placement code, all of them corrections:
//!
//! * A caret rectangle with zero width but real height is valid. `GetTextExt` on
//!   a collapsed range normally returns exactly that; Go treated it as invalid
//!   and put the list at the mouse pointer.
//! * When the list does not fit below the caret it goes above the caret's *top*.
//!   Go flipped around the caret's bottom edge, which covered the line being typed.
//! * The list is kept inside the monitor's work area rather than the whole
//!   virtual screen, so it cannot straddle two monitors of different sizes.

use crate::engine::Candidate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

/// The numbered list the window shows, one line per candidate, with a marker in
/// front of the selected one.
pub fn format_lines(candidates: &[Candidate], selected: usize) -> Vec<String> {
    candidates
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}{}. {}", if i == selected { "\u{25B6} " } else { "  " }, i + 1, c.text))
        .collect()
}

/// Scales a 96-DPI pixel value to `dpi`.
pub fn scale(value: i32, dpi: u32) -> i32 {
    let dpi = if dpi == 0 { 96 } else { dpi } as i64;
    ((value as i64 * dpi + 48) / 96) as i32
}

/// A caret rectangle is usable when it has height; width may be zero.
pub fn caret_is_valid(caret: [i32; 4]) -> bool {
    caret[2] >= caret[0] && caret[3] > caret[1]
}

/// Where the list wants to sit: just below the insertion point, or above it if
/// there is no room. Returns (x, y below, y above).
pub fn anchor(caret: [i32; 4], cursor: Option<(i32, i32)>) -> (i32, i32, i32) {
    if caret_is_valid(caret) {
        (caret[0], caret[3], caret[1])
    } else {
        // No usable rectangle: sit at the mouse pointer, or in a corner.
        let (x, y) = cursor.unwrap_or((16, 16));
        (x, y, y)
    }
}

/// Top-left corner for a window of `size` (width, height), kept inside `bounds`.
pub fn place_window(caret: [i32; 4], cursor: Option<(i32, i32)>, size: (i32, i32), bounds: Rect) -> (i32, i32) {
    let (width, height) = size;
    let (mut left, below, above) = anchor(caret, cursor);
    let mut top = below;
    if left + width > bounds.right {
        left = bounds.right - width;
    }
    if left < bounds.left {
        left = bounds.left;
    }
    if top + height > bounds.bottom {
        top = above - height;
    }
    if top < bounds.top {
        top = bounds.top;
    }
    (left, top)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };

    fn cand(text: &str) -> Candidate {
        Candidate { text: text.into(), index: 0, annotation: String::new() }
    }

    #[test]
    fn lines_are_numbered_and_the_selection_is_marked() {
        let lines = format_lines(&[cand("你好"), cand("拟好"), cand("泥")], 1);
        assert_eq!(lines, ["  1. 你好", "\u{25B6} 2. 拟好", "  3. 泥"]);
        assert!(format_lines(&[], 0).is_empty());
        // a selection past the end marks nothing
        assert!(format_lines(&[cand("a")], 5)[0].starts_with("  1."));
    }

    #[test]
    fn scaling_follows_dpi() {
        assert_eq!(scale(10, 96), 10);
        assert_eq!(scale(10, 144), 15);
        assert_eq!(scale(10, 192), 20);
        assert_eq!(scale(10, 0), 10, "an unknown DPI is 96");
        assert_eq!(scale(7, 120), 9);
    }

    #[test]
    fn a_zero_width_caret_with_height_is_valid() {
        assert!(caret_is_valid([100, 200, 100, 220]), "a collapsed range has no width");
        assert!(caret_is_valid([100, 200, 102, 220]));
        assert!(!caret_is_valid([0, 0, 0, 0]));
        assert!(!caret_is_valid([100, 200, 110, 200]), "no height");
        assert!(!caret_is_valid([100, 200, 90, 220]), "inverted");
    }

    #[test]
    fn the_list_goes_below_the_caret() {
        let at = place_window([100, 200, 100, 220], Some((5, 5)), (200, 150), SCREEN);
        assert_eq!(at, (100, 220), "below the caret, not at the mouse");
    }

    #[test]
    fn the_list_flips_above_the_caret_top_not_its_bottom() {
        // 1080 high screen, caret near the bottom, 150 high list
        let at = place_window([100, 1040, 100, 1060], None, (200, 150), SCREEN);
        assert_eq!(at, (100, 1040 - 150), "the list's bottom edge meets the caret's top");
        assert!(at.1 + 150 <= 1040, "the typing line stays visible");
    }

    #[test]
    fn the_list_is_pulled_back_inside_the_right_and_left_edges() {
        assert_eq!(place_window([1900, 100, 1900, 120], None, (300, 100), SCREEN).0, 1920 - 300);
        assert_eq!(place_window([-50, 100, -50, 120], None, (300, 100), SCREEN).0, 0);
        // wider than the screen: left edge wins
        assert_eq!(place_window([10, 100, 10, 120], None, (3000, 100), SCREEN).0, 0);
    }

    #[test]
    fn the_top_edge_wins_when_nothing_fits() {
        let short = Rect { left: 0, top: 0, right: 800, bottom: 100 };
        assert_eq!(place_window([10, 40, 10, 60], None, (100, 300), short).1, 0);
    }

    #[test]
    fn an_invalid_caret_falls_back_to_the_pointer_then_a_corner() {
        assert_eq!(place_window([0; 4], Some((640, 480)), (100, 100), SCREEN), (640, 480));
        assert_eq!(place_window([0; 4], None, (100, 100), SCREEN), (16, 16));
        // at the pointer near the bottom it flips above the pointer
        assert_eq!(place_window([0; 4], Some((640, 1070)), (100, 100), SCREEN), (640, 970));
    }

    #[test]
    fn a_second_monitor_with_a_negative_origin_is_respected() {
        let left_monitor = Rect { left: -1280, top: 0, right: 0, bottom: 1024 };
        let at = place_window([-100, 500, -100, 520], None, (300, 100), left_monitor);
        assert_eq!(at, (-300, 520), "clamped to the monitor the caret is on");
    }

    #[test]
    fn rect_dimensions() {
        assert_eq!((SCREEN.width(), SCREEN.height()), (1920, 1080));
    }
}
