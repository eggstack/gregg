#![allow(dead_code)]

use crate::state::{AppState, Pane};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Render a message when no systems are configured.
pub fn render_empty_config(f: &mut Frame, area: Rect) {
    let msg = "No sources configured. Use: gregg add HOST[:PORT] or gregg eggpool add HOST[:PORT]";
    let paragraph =
        Paragraph::new(Line::from(Span::raw(msg))).style(Style::default().fg(Color::Yellow));
    f.render_widget(paragraph, area);
}

/// Render a message when the terminal is too small.
pub fn render_too_small(f: &mut Frame, area: Rect) {
    let msg = "gregg: terminal too small";
    let paragraph =
        Paragraph::new(Line::from(Span::raw(msg))).style(Style::default().fg(Color::Red));
    f.render_widget(paragraph, area);
}

/// Render a one-line key hint in the last row when vertical space permits.
pub fn render_key_hint(f: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let hint_area = Rect {
        y: area.y.saturating_add(area.height).saturating_sub(1),
        height: 1,
        ..area
    };
    let both = !state.systems.is_empty() && state.eggpool.is_some();
    let hint = if state.active_pane == Pane::Eggpool {
        if both {
            "h/l:pane  j/k:window  Ctrl-R:refresh  q:quit"
        } else {
            "j/k:window  Ctrl-R:refresh  q:quit"
        }
    } else if area.width < 60 {
        if both {
            "j/k select  h/l pane  v view  e drives  q quit"
        } else {
            "j/k select  v view  e drives  q quit"
        }
    } else {
        if both {
            "j/k:select  h/l:pane  v:view  e:drives  Ctrl-R:refresh  q:quit"
        } else {
            "j/k:select  v:view  e:drives  Ctrl-R:refresh  q:quit"
        }
    };
    let paragraph =
        Paragraph::new(Line::from(Span::raw(hint))).style(Style::default().fg(Color::DarkGray));
    f.render_widget(paragraph, hint_area);
}
