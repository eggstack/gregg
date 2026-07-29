#![allow(dead_code)]

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::normalized::NormalizedSnapshot;
use crate::state::SystemState;

use super::bar;
use super::text;

/// Render a 4-row online system block.
pub fn render_online(f: &mut Frame, area: Rect, system: &SystemState, is_selected: bool) {
    if area.height < 4 || area.width == 0 {
        return;
    }

    let Some(snap) = &system.latest else {
        return;
    };

    let sel_style = if is_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    // Row 0: header line
    let header = text::header_line(system, area.width);
    let header_line = Line::from(Span::styled(header, sel_style));
    f.render_widget(header_line, Rect { height: 1, ..area });

    // Row 1: CPU bar
    let cpu_detail = format!("{} cores", snap.logical_cores);
    bar::render_bar(
        f,
        Rect {
            y: area.y.saturating_add(1),
            height: 1,
            ..area
        },
        "CPU",
        snap.usage_pct,
        Some(&cpu_detail),
        false,
    );

    // Row 2: MEM bar
    let mem_detail = format!(
        "{}/{}",
        text::format_bytes(snap.memory.used_bytes),
        text::format_bytes(snap.memory.total_bytes)
    );
    bar::render_bar(
        f,
        Rect {
            y: area.y.saturating_add(2),
            height: 1,
            ..area
        },
        "MEM",
        snap.memory.usage_pct,
        Some(&mem_detail),
        false,
    );

    // Row 3: SWAP or COMMIT bar
    render_swap_or_commit_row(f, area, snap);
}

/// Render the third row: SWAP, COMMIT, or unavailable.
fn render_swap_or_commit_row(f: &mut Frame, area: Rect, snap: &NormalizedSnapshot) {
    if let Some(ref swap) = snap.swap {
        let swap_detail = if swap.total_bytes == 0 {
            String::new()
        } else {
            format!(
                "{}/{}",
                text::format_bytes(swap.used_bytes),
                text::format_bytes(swap.total_bytes)
            )
        };
        bar::render_bar(
            f,
            Rect {
                y: area.y.saturating_add(3),
                height: 1,
                ..area
            },
            "SWP",
            swap.usage_pct,
            if swap_detail.is_empty() {
                None
            } else {
                Some(&swap_detail)
            },
            false,
        );
    } else if let Some(ref commit) = snap.commit {
        let commit_detail = format!(
            "{}/{}",
            text::format_bytes(commit.used_bytes),
            text::format_bytes(commit.limit_bytes)
        );
        bar::render_bar(
            f,
            Rect {
                y: area.y.saturating_add(3),
                height: 1,
                ..area
            },
            "COMMIT",
            commit.usage_pct,
            Some(&commit_detail),
            false,
        );
    } else {
        // Neither swap nor commit - show unavailable
        bar::render_bar(
            f,
            Rect {
                y: area.y.saturating_add(3),
                height: 1,
                ..area
            },
            "SWP",
            0.0,
            None,
            false,
        );
    }
}

/// Render a 1-row offline system line.
#[allow(clippy::cast_possible_truncation)]
pub fn render_offline(f: &mut Frame, area: Rect, system: &SystemState, is_selected: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let style = if is_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    let display_name = system
        .configured_name
        .as_deref()
        .unwrap_or(&system.endpoint.host);

    let status_text = match system.reachability {
        crate::state::Reachability::Offline => "offline",
        crate::state::Reachability::Pending => "pending",
        crate::state::Reachability::Online => "online",
    };

    let label_width =
        (display_name.len() + system.endpoint.display_address().len() + status_text.len() + 3)
            as u16;
    let available = area.width.saturating_sub(label_width);
    let dot = if available > 0 {
        ".".repeat(available as usize)
    } else {
        String::new()
    };

    let line = Line::from(vec![
        Span::styled(
            format!(
                "{}@{} {status_text} ",
                display_name,
                system.endpoint.display_address()
            ),
            style,
        ),
        Span::styled(dot, style),
    ]);

    f.render_widget(line, area);
}
