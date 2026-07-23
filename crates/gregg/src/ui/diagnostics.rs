#![allow(dead_code)]

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Render a message when no systems are configured.
pub fn render_empty_config(f: &mut Frame, area: Rect) {
    let msg = "No systems configured. Use: gregg add HOST[:PORT]";
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
