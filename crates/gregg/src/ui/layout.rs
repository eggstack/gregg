#![allow(dead_code)]

use ratatui::layout::Rect;

use crate::state::{entry_height, visible_range, AppState};

/// A single entry in the viewport with its rect and selection state.
pub struct ViewportEntry {
    pub index: usize,
    pub rect: Rect,
    pub is_selected: bool,
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

    let visible = visible_range(&display_order, &state.systems, top_pos, area.height);

    let mut entries = Vec::new();
    let mut y = area.y;

    for idx in visible {
        if idx >= display_order.len() {
            break;
        }
        let sys_idx = display_order[idx];
        let system = &state.systems[sys_idx];
        let h = entry_height(system);
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

        entries.push(ViewportEntry {
            index: sys_idx,
            rect,
            is_selected,
        });

        y = y.saturating_add(h);
    }

    entries
}
