#![allow(dead_code)]

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::text;

/// Render a reusable usage bar.
///
/// Format: `CPU  [||||||||        ] 25.2% 8 cores`
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn render_bar(
    f: &mut Frame,
    area: Rect,
    label: &str,
    pct: f32,
    detail: Option<&str>,
    _is_selected: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let clamped_pct = pct.clamp(0.0, 100.0);

    let label_len = (label.len() as u16).min(area.width);
    let min_reserve = label_len + 1 + 1 + 1 + 1 + 1 + 6;
    let bar_width = area.width.saturating_sub(min_reserve);

    let filled = if bar_width > 0 {
        ((clamped_pct / 100.0) * f32::from(bar_width)) as u16
    } else {
        0
    };
    let empty = bar_width.saturating_sub(filled);

    let bar_chars: String = "|".repeat(filled as usize);
    let space_chars: String = " ".repeat(empty as usize);

    let pct_str = text::format_pct(clamped_pct);

    let mut spans = vec![Span::raw(format!(
        "{label}  [{bar_chars}{space_chars}] {pct_str}"
    ))];

    if let Some(d) = detail {
        let detail_budget = area.width.saturating_sub(min_reserve);
        if detail_budget >= 2 {
            let truncated = truncate_str(d, detail_budget);
            spans.push(Span::raw(format!(" {truncated}")));
        }
    }

    let line = Line::from(spans);
    f.render_widget(line, area);
}

/// Truncate a string to at most `max_chars` characters.
fn truncate_str(s: &str, max_chars: u16) -> String {
    let max = max_chars as usize;
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}
