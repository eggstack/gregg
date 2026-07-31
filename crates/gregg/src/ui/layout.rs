#![allow(dead_code)]

use ratatui::layout::Rect;

use crate::state::{entry_height, visible_range, AppState};

/// A single entry in the viewport with its rect and selection state.
pub struct ViewportEntry {
    pub index: usize,
    pub rect: Rect,
    pub is_selected: bool,
    pub drive_rows_visible: usize,
}

/// Compute which systems are visible and their rect positions.
pub fn compute_viewport(state: &AppState, area: Rect) -> Vec<ViewportEntry> {
    let display_order = state.display_order();

    let top_pos = state
        .viewport_top_id
        .as_ref()
        .and_then(|top| {
            display_order
                .iter()
                .position(|&i| state.systems[i].id == *top)
        })
        .unwrap_or(0);

    let visible = visible_range(&display_order, state, top_pos, area.height);

    let mut entries = Vec::new();
    let mut y = area.y;

    for idx in visible {
        if idx >= display_order.len() {
            break;
        }
        let sys_idx = display_order[idx];
        let system = &state.systems[sys_idx];
        let full_height = entry_height(state, sys_idx);
        let height_remaining = area.y.saturating_add(area.height).saturating_sub(y);
        let h = full_height.min(height_remaining);
        let is_selected = state
            .selected_id
            .as_deref()
            .is_some_and(|sel| system.id == *sel);

        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: h,
        };

        let drive_rows_visible = if is_selected && state.drives_expanded {
            usize::from(h.saturating_sub(5))
        } else {
            0
        };

        entries.push(ViewportEntry {
            index: sys_idx,
            rect,
            is_selected,
            drive_rows_visible,
        });

        y = y.saturating_add(h);
    }

    entries
}
