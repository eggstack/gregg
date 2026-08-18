#![allow(dead_code)]

//! Low-level text and rendering helpers shared by `system_block` and other
//! views.
//!
//! These primitives intentionally keep no per-row layout state. Each `ui`
//! module is responsible for computing its own label, bar, and suffix
//! widths so that all sibling rows align. The helpers here provide
//! display-cell-aware truncation plus a tiny line renderer that the
//! higher-level layout code drives.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

/// Truncate a string to at most `max_width` display cells.
///
/// If the source already fits, it is returned unchanged. When truncation
/// occurs the returned string is the longest prefix whose terminal-cell
/// width does not exceed `max_width` after appending an ellipsis; the
/// ellipsis is dropped whenever it would itself overflow the budget so
/// the caller can rely on `width(truncate_to_cells(s, n)) <= n`.
#[must_use]
pub fn truncate_to_cells(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut width = 0usize;
    let mut end = 0usize;
    for (i, ch) in s.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > max_width {
            break;
        }
        width += w;
        end = i + ch.len_utf8();
    }
    if end >= s.len() {
        // The source already fits exactly.
        s.to_string()
    } else if width < max_width {
        // Room for the ellipsis after the last kept character.
        format!("{}…", &s[..end])
    } else if end > 0 {
        // Even an ellipsis would overflow; return what fits.
        s[..end].to_string()
    } else {
        String::new()
    }
}

/// Render one pre-built text line into a single-row area.
///
/// The caller is responsible for ensuring `line` already fits inside
/// `area.width`; this helper does not truncate or wrap.
pub fn render_text_line(f: &mut Frame, area: Rect, line: &str) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    f.render_widget(Line::from(Span::raw(line.to_string())), area);
}
