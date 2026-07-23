#![allow(dead_code)]

//! Raw input and signal events for the TUI event loop.
//!
//! Events represent external occurrences that the TUI loop receives and
//! translates into [`Action`](crate::action::Action)s before applying
//! them to [`AppState`](crate::state::AppState). Separating raw events
//! from actions keeps the translation logic testable and the state
//! reducer pure.

use crate::poller::PollBatch;

/// A raw event from the terminal, OS signals, or internal sources.
///
/// The TUI loop receives events and maps them to typed
/// [`Action`](crate::action::Action)s. This indirection lets the
/// state engine remain independent of input handling details.
pub enum Event {
    /// A key was pressed on the terminal.
    KeyInput(KeyEvent),
    /// The terminal was resized.
    Resize {
        /// New width in columns.
        width: u16,
        /// New height in rows.
        height: u16,
    },
    /// An OS signal was received.
    Signal(SignalKind),
    /// A poll batch arrived from the scheduler.
    BatchReceived(PollBatch),
    /// The configuration file changed on disk.
    ConfigChanged,
}

/// A key event from the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// The key that was pressed.
    pub key: Key,
    /// Whether the Control modifier was held.
    pub ctrl: bool,
    /// Whether the Alt/Option modifier was held.
    pub alt: bool,
    /// Whether the Shift modifier was held.
    pub shift: bool,
}

/// A terminal key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A Unicode character.
    Char(char),
    /// The Enter/Return key.
    Enter,
    /// The Escape key.
    Esc,
    /// The Tab key.
    Tab,
    /// The Backspace key.
    Backspace,
    /// The Delete key.
    Delete,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page Up key.
    PageUp,
    /// Page Down key.
    PageDown,
}

/// An OS signal that the TUI loop handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    /// SIGHUP — reload configuration.
    Hangup,
    /// SIGWINCH — terminal resized (on platforms that deliver it).
    WindowChange,
    /// SIGTERM / SIGINT — graceful shutdown.
    Terminate,
}

/// Translate a [`KeyEvent`] into an [`Action`](crate::action::Action).
///
/// Returns `None` for keys that do not map to an action.
#[must_use]
pub fn key_to_action(event: KeyEvent) -> Option<crate::action::Action> {
    use crate::action::Action;

    if event.ctrl {
        return match event.key {
            Key::Char('c') => Some(Action::Quit),
            Key::Char('r') => Some(Action::RefreshNow),
            _ => None,
        };
    }

    match event.key {
        Key::Char('j') | Key::Down => Some(Action::SelectNext),
        Key::Char('k') | Key::Up => Some(Action::SelectPrevious),
        Key::Char('g') if !event.shift => Some(Action::SelectFirst),
        Key::Char('G') => Some(Action::SelectLast),
        Key::PageDown | Key::Char('f') if !event.ctrl => Some(Action::PageDown),
        Key::PageUp | Key::Char('b') if !event.ctrl => Some(Action::PageUp),
        Key::Char('q') | Key::Esc => Some(Action::Quit),
        _ => None,
    }
}

/// Translate an [`Event`] into zero or more [`Action`](crate::action::Action)s.
///
/// Most events produce a single action. Some (like a signal) may produce
/// none if the caller handles them directly.
#[must_use]
pub fn translate_event(event: &Event) -> Option<crate::action::Action> {
    match event {
        Event::KeyInput(key_event) => key_to_action(*key_event),
        Event::Resize { width, height } => Some(crate::action::Action::Resize {
            width: *width,
            height: *height,
        }),
        Event::Signal(SignalKind::Terminate) => Some(crate::action::Action::Quit),
        Event::Signal(SignalKind::Hangup | SignalKind::WindowChange)
        | Event::BatchReceived(_)
        | Event::ConfigChanged => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_variants_exist() {
        let keys = [
            Key::Char('a'),
            Key::Enter,
            Key::Esc,
            Key::Tab,
            Key::Backspace,
            Key::Delete,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
        ];
        assert_eq!(keys.len(), 14);
        assert_eq!(keys[0], Key::Char('a'));
        assert_eq!(keys[6], Key::Up);
    }

    #[test]
    fn event_variants_exist() {
        let events = [
            Event::KeyInput(KeyEvent {
                key: Key::Char('j'),
                ctrl: false,
                alt: false,
                shift: false,
            }),
            Event::Resize {
                width: 80,
                height: 24,
            },
            Event::Signal(SignalKind::Hangup),
            Event::BatchReceived(crate::poller::PollBatch {
                generation: 1,
                started_at: std::time::Instant::now(),
                completed_at: std::time::Instant::now(),
                results: vec![],
            }),
            Event::ConfigChanged,
        ];
        assert_eq!(events.len(), 5);
    }

    #[test]
    fn key_to_action_j_and_k() {
        let action = key_to_action(KeyEvent {
            key: Key::Char('j'),
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::SelectNext)));

        let action = key_to_action(KeyEvent {
            key: Key::Char('k'),
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(
            action,
            Some(crate::action::Action::SelectPrevious)
        ));
    }

    #[test]
    fn key_to_action_arrow_keys() {
        let action = key_to_action(KeyEvent {
            key: Key::Down,
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::SelectNext)));

        let action = key_to_action(KeyEvent {
            key: Key::Up,
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(
            action,
            Some(crate::action::Action::SelectPrevious)
        ));
    }

    #[test]
    fn key_to_action_ctrl_c_quits() {
        let action = key_to_action(KeyEvent {
            key: Key::Char('c'),
            ctrl: true,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::Quit)));
    }

    #[test]
    fn key_to_action_ctrl_r_refreshes() {
        let action = key_to_action(KeyEvent {
            key: Key::Char('r'),
            ctrl: true,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::RefreshNow)));
    }

    #[test]
    fn key_to_action_gg_selects_first() {
        let action = key_to_action(KeyEvent {
            key: Key::Char('g'),
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::SelectFirst)));
    }

    #[test]
    fn key_to_action_shift_g_selects_last() {
        let action = key_to_action(KeyEvent {
            key: Key::Char('G'),
            ctrl: false,
            alt: false,
            shift: true,
        });
        assert!(matches!(action, Some(crate::action::Action::SelectLast)));
    }

    #[test]
    fn key_to_action_page_down_and_up() {
        let action = key_to_action(KeyEvent {
            key: Key::PageDown,
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::PageDown)));

        let action = key_to_action(KeyEvent {
            key: Key::PageUp,
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::PageUp)));
    }

    #[test]
    fn key_to_action_f_and_b() {
        let action = key_to_action(KeyEvent {
            key: Key::Char('f'),
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::PageDown)));

        let action = key_to_action(KeyEvent {
            key: Key::Char('b'),
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::PageUp)));
    }

    #[test]
    fn key_to_action_q_quits() {
        let action = key_to_action(KeyEvent {
            key: Key::Char('q'),
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::Quit)));
    }

    #[test]
    fn key_to_action_esc_quits() {
        let action = key_to_action(KeyEvent {
            key: Key::Esc,
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(matches!(action, Some(crate::action::Action::Quit)));
    }

    #[test]
    fn key_to_action_unmapped_returns_none() {
        let action = key_to_action(KeyEvent {
            key: Key::Char('x'),
            ctrl: false,
            alt: false,
            shift: false,
        });
        assert!(action.is_none());
    }

    #[test]
    fn translate_event_resize() {
        let event = Event::Resize {
            width: 120,
            height: 40,
        };
        let action = translate_event(&event);
        assert!(matches!(
            action,
            Some(crate::action::Action::Resize {
                width: 120,
                height: 40
            })
        ));
    }

    #[test]
    fn translate_event_signal_terminate() {
        let event = Event::Signal(SignalKind::Terminate);
        let action = translate_event(&event);
        assert!(matches!(action, Some(crate::action::Action::Quit)));
    }

    #[test]
    fn translate_event_signal_hangup_returns_none() {
        let event = Event::Signal(SignalKind::Hangup);
        let action = translate_event(&event);
        assert!(action.is_none());
    }

    #[test]
    fn translate_event_batch_returns_none() {
        let event = Event::BatchReceived(crate::poller::PollBatch {
            generation: 1,
            started_at: std::time::Instant::now(),
            completed_at: std::time::Instant::now(),
            results: vec![],
        });
        let action = translate_event(&event);
        assert!(action.is_none());
    }

    #[test]
    fn signal_kind_variants() {
        assert_eq!(SignalKind::Hangup, SignalKind::Hangup);
        assert_eq!(SignalKind::WindowChange, SignalKind::WindowChange);
        assert_eq!(SignalKind::Terminate, SignalKind::Terminate);
        assert_ne!(SignalKind::Hangup, SignalKind::Terminate);
    }

    #[test]
    fn key_event_modifiers() {
        let event = KeyEvent {
            key: Key::Char('a'),
            ctrl: true,
            alt: true,
            shift: true,
        };
        assert!(event.ctrl);
        assert!(event.alt);
        assert!(event.shift);
    }
}
