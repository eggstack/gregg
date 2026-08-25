#![allow(dead_code)]

use ratatui::layout::Rect;

use crate::state::{entry_height, view_header_height, visible_range, AppState};

/// A single entry in the viewport with its rect and selection state.
pub struct ViewportEntry {
    pub index: usize,
    pub rect: Rect,
    /// Plan 087: logical selection (the persistent selected system
    /// ID). Drives drive-detail row allocation and the `e` action.
    pub is_selected: bool,
    /// Plan 087: visual selection. `true` only when the entry is
    /// logically selected AND `AppState::selection_highlight_active`.
    /// Drives the reversed-video styling in the renderer.
    pub is_visually_selected: bool,
    pub drive_rows_visible: usize,
}

/// Compute which systems are visible and their rect positions.
///
/// `display_order` must be the state's current display order; passing a
/// precomputed order lets one render reuse a single allocation across the
/// layout and its callers.
pub fn compute_viewport(
    state: &AppState,
    area: Rect,
    display_order: &[usize],
) -> Vec<ViewportEntry> {
    let top_pos = state
        .viewport_top_id
        .as_ref()
        .and_then(|top| {
            display_order
                .iter()
                .position(|&i| state.systems[i].id == *top)
        })
        .unwrap_or(0);

    let header_height = view_header_height(state.system_view_mode);
    let content_area = ratatui::layout::Rect {
        y: area.y.saturating_add(header_height),
        height: area.height.saturating_sub(header_height),
        ..area
    };
    let visible = visible_range(display_order, state, top_pos, content_area.height);

    let mut entries = Vec::new();
    let mut y = content_area.y;

    for idx in visible {
        if idx >= display_order.len() {
            break;
        }
        let sys_idx = display_order[idx];
        let system = &state.systems[sys_idx];
        let full_height = entry_height(state, sys_idx);
        let height_remaining = content_area
            .y
            .saturating_add(content_area.height)
            .saturating_sub(y);
        let h = full_height.min(height_remaining);
        let is_selected = state
            .selected_id
            .as_deref()
            .is_some_and(|sel| system.id == *sel);
        let is_visually_selected = is_selected && state.selection_highlight_active;

        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: h,
        };

        let base_height = match state.system_view_mode {
            crate::state::SystemViewMode::Normal => 5,
            crate::state::SystemViewMode::Condensed => 1,
        };
        // Plan 087: drive-detail visibility is tied to logical
        // selection, not the transient visual highlight. An expanded
        // drive list must survive a highlight timeout.
        let drive_rows_visible = if is_selected && state.drives_expanded {
            usize::from(h.saturating_sub(base_height))
        } else {
            0
        };

        entries.push(ViewportEntry {
            index: sys_idx,
            rect,
            is_selected,
            is_visually_selected,
            drive_rows_visible,
        });

        y = y.saturating_add(h);
    }

    entries
}
