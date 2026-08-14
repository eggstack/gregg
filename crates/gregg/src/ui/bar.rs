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
    let pct_str = text::format_pct(clamped_pct);
    let label_len = (label.len() as u16).min(area.width);
    let fixed_width = label_len
        .saturating_add(2)
        .saturating_add(2)
        .saturating_add(1)
        .saturating_add(u16::try_from(pct_str.len()).unwrap_or(u16::MAX));
    let detail = detail.map(|value| {
        let budget = usize::from(area.width.saturating_sub(fixed_width.saturating_add(1))) / 2;
        truncate_str(value, u16::try_from(budget).unwrap_or(u16::MAX))
    });
    let detail_width = detail.as_deref().map_or(0, |value| {
        u16::try_from(value.len() + 1).unwrap_or(u16::MAX)
    });
    let bar_width = area
        .width
        .saturating_sub(fixed_width.saturating_add(detail_width));

    let filled = if bar_width > 0 {
        ((clamped_pct / 100.0) * f32::from(bar_width)) as u16
    } else {
        0
    };
    let empty = bar_width.saturating_sub(filled);

    let bar_chars: String = "|".repeat(filled as usize);
    let space_chars: String = " ".repeat(empty as usize);

    let mut spans = vec![Span::raw(format!(
        "{label}  [{bar_chars}{space_chars}] {pct_str}"
    ))];

    if let Some(d) = detail {
        spans.push(Span::raw(format!(" {d}")));
    }

    let line = Line::from(spans);
    f.render_widget(line, area);
}

/// Render a metric whose value is unavailable.
pub fn render_unavailable(f: &mut Frame, area: Rect, label: &str) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let fixed_width = label.len().saturating_add(4).saturating_add(3);
    let empty_width = usize::from(area.width).saturating_sub(fixed_width);
    let line = format!("{label}  [{}] —", " ".repeat(empty_width));
    f.render_widget(Line::from(line), area);
}

/// Truncate a string to at most `max_width` display columns.
fn truncate_str(s: &str, max_width: u16) -> String {
    use unicode_width::UnicodeWidthChar;

    let max = max_width as usize;
    let mut width = 0usize;
    let mut end = 0usize;
    for (i, ch) in s.char_indices() {
        let w = ch.width().unwrap_or(0);
        if width + w > max {
            break;
        }
        width += w;
        end = i + ch.len_utf8();
    }
    if end >= s.len() {
        s.to_string()
    } else if end > 0 {
        format!("{}…", &s[..end])
    } else {
        String::new()
    }
}
