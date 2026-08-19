#![allow(dead_code)]

//! User and system actions for typed state transitions.
//!
//! Actions represent every event that can mutate [`AppState`](crate::state::AppState).
//! Separating actions from state mutations makes the reducer pure and testable.

/// A typed event that triggers a state transition.
///
/// Actions are produced by input handlers (keyboard, signal) and the
/// scheduler. The [`AppState::apply_action`](crate::state::AppState::apply_action)
/// method consumes actions and mutates state deterministically.
#[derive(Clone, Copy)]
pub enum Action {
    /// Move down in the active pane.
    MoveDown,
    /// Move up in the active pane.
    MoveUp,
    /// Move selection down by approximately one viewport.
    PageDown,
    /// Move selection up by approximately one viewport.
    PageUp,
    /// Move selection to the first system in display order.
    SelectFirst,
    /// Move selection to the last system in display order.
    SelectLast,
    /// Select the previous available top-level pane.
    PreviousPane,
    /// Select the next available top-level pane.
    NextPane,
    /// Toggle the Systems presentation mode.
    ToggleSystemView,
    /// Toggle drive details for the selected system.
    ToggleDrives,
    /// Trigger an immediate poll cycle (handled by the scheduler).
    RefreshNow,
    /// Plan 087: drop the visual selection highlight (the reversed
    /// styling) without touching logical `selected_id`. Produced by the
    /// event-loop timer about ten seconds after the most recent
    /// selection-changing Systems action.
    ClearSelectionHighlight,
    /// The terminal was resized.
    Resize {
        /// New width in columns.
        width: u16,
        /// New height in rows.
        height: u16,
    },
    /// Exit the application.
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_variants_exist() {
        let actions = [
            Action::MoveDown,
            Action::MoveUp,
            Action::PageDown,
            Action::PageUp,
            Action::SelectFirst,
            Action::SelectLast,
            Action::PreviousPane,
            Action::NextPane,
            Action::ToggleSystemView,
            Action::ToggleDrives,
            Action::RefreshNow,
            Action::ClearSelectionHighlight,
            Action::Resize {
                width: 80,
                height: 24,
            },
            Action::Quit,
        ];
        // Verify all variants are constructible and match.
        assert!(matches!(actions[0], Action::MoveDown));
        assert!(matches!(actions[1], Action::MoveUp));
        assert!(matches!(actions[2], Action::PageDown));
        assert!(matches!(actions[3], Action::PageUp));
        assert!(matches!(actions[4], Action::SelectFirst));
        assert!(matches!(actions[5], Action::SelectLast));
        assert!(matches!(actions[6], Action::PreviousPane));
        assert!(matches!(actions[7], Action::NextPane));
        assert!(matches!(actions[8], Action::ToggleSystemView));
        assert!(matches!(actions[9], Action::ToggleDrives));
        assert!(matches!(actions[10], Action::RefreshNow));
        assert!(matches!(actions[11], Action::ClearSelectionHighlight));
        assert!(matches!(
            actions[12],
            Action::Resize {
                width: 80,
                height: 24
            }
        ));
        assert!(matches!(actions[13], Action::Quit));
    }

    #[test]
    fn resize_carries_dimensions() {
        let action = Action::Resize {
            width: 120,
            height: 40,
        };
        match action {
            Action::Resize { width, height } => {
                assert_eq!(width, 120);
                assert_eq!(height, 40);
            }
            _ => panic!("expected Resize"),
        }
    }
}
