#![allow(dead_code)]

//! User and system actions for typed state transitions.
//!
//! Actions represent every event that can mutate [`AppState`](crate::state::AppState).
//! Separating actions from state mutations makes the reducer pure and testable.

use crate::config::Config;

/// A typed event that triggers a state transition.
///
/// Actions are produced by input handlers (keyboard, signal) and the
/// scheduler. The [`AppState::apply_action`](crate::state::AppState::apply_action)
/// method consumes actions and mutates state deterministically.
pub enum Action {
    /// Move selection to the next system in display order.
    SelectNext,
    /// Move selection to the previous system in display order.
    SelectPrevious,
    /// Move selection down by approximately one viewport.
    PageDown,
    /// Move selection up by approximately one viewport.
    PageUp,
    /// Move selection to the first system in display order.
    SelectFirst,
    /// Move selection to the last system in display order.
    SelectLast,
    /// Trigger an immediate poll cycle (handled by the scheduler).
    RefreshNow,
    /// The configuration was reloaded; rebuild state from the new config.
    ConfigReloaded(Config),
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
    use crate::config::{Config, SystemEntry};

    #[test]
    fn action_variants_exist() {
        let actions = [
            Action::SelectNext,
            Action::SelectPrevious,
            Action::PageDown,
            Action::PageUp,
            Action::SelectFirst,
            Action::SelectLast,
            Action::RefreshNow,
            Action::ConfigReloaded(Config::default()),
            Action::Resize {
                width: 80,
                height: 24,
            },
            Action::Quit,
        ];
        // Verify all variants are constructible and match.
        assert!(matches!(actions[0], Action::SelectNext));
        assert!(matches!(actions[1], Action::SelectPrevious));
        assert!(matches!(actions[2], Action::PageDown));
        assert!(matches!(actions[3], Action::PageUp));
        assert!(matches!(actions[4], Action::SelectFirst));
        assert!(matches!(actions[5], Action::SelectLast));
        assert!(matches!(actions[6], Action::RefreshNow));
        assert!(matches!(actions[7], Action::ConfigReloaded(_)));
        assert!(matches!(
            actions[8],
            Action::Resize {
                width: 80,
                height: 24
            }
        ));
        assert!(matches!(actions[9], Action::Quit));
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

    #[test]
    fn config_reloaded_carry_config() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "test-id".into(),
            host: "192.168.1.1".into(),
            port: 11310,
            name: None,
        });
        let action = Action::ConfigReloaded(config);
        match action {
            Action::ConfigReloaded(c) => {
                assert_eq!(c.systems.len(), 1);
                assert_eq!(c.systems[0].host, "192.168.1.1");
            }
            _ => panic!("expected ConfigReloaded"),
        }
    }
}
