//! Crossterm event stream adapter.
//!
//! Reads crossterm events in a dedicated thread and sends them as
//! typed [`Event`]s through a bounded channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::event::{Event, Key, KeyEvent as CrateKeyEvent};

/// Background reader that converts crossterm events into [`Event`]s and
/// forwards them through a bounded channel.
#[allow(dead_code)]
pub struct EventStream {
    shutdown: Arc<AtomicBool>,
    _handle: std::thread::JoinHandle<()>,
}

impl EventStream {
    /// Create a new event stream. Returns the handle and a receiving half
    /// of a bounded channel (capacity 32).
    pub fn new() -> (Self, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel(32);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
            rt.block_on(async {
                let mut stream = crossterm::event::EventStream::new();
                loop {
                    tokio::select! {
                        () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                            if shutdown_clone.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                        next_event = stream.next() => {
                            match next_event {
                                Some(Ok(crossterm_event)) => {
                                    let converted = convert_crossterm_event(&crossterm_event);
                                    if let Some(event) = converted {
                                        if tx.send(event).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                Some(Err(_)) | None => break,
                            }
                        }
                    }
                }
            });
        });

        (
            Self {
                shutdown,
                _handle: handle,
            },
            rx,
        )
    }

    /// Signal the background thread to stop. Dropping the `EventStream`
    /// also triggers shutdown.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Convert a crossterm [`crossterm::event::Event`] into our crate's
/// [`Event`], returning `None` for unhandled event types.
#[allow(dead_code)]
fn convert_crossterm_event(event: &crossterm::event::Event) -> Option<Event> {
    match event {
        crossterm::event::Event::Key(key_event) => {
            convert_crossterm_key(*key_event).map(Event::KeyInput)
        }
        crossterm::event::Event::Resize(width, height) => Some(Event::Resize {
            width: *width,
            height: *height,
        }),
        _ => None,
    }
}

/// Convert a crossterm [`crossterm::event::KeyEvent`] into our crate's
/// [`CrateKeyEvent`], returning `None` for unmapped key codes.
#[allow(dead_code)]
fn convert_crossterm_key(ck: crossterm::event::KeyEvent) -> Option<CrateKeyEvent> {
    use crossterm::event::KeyCode;

    let key = match ck.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        _ => return None,
    };

    Some(CrateKeyEvent {
        key,
        ctrl: ck
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL),
        alt: ck.modifiers.contains(crossterm::event::KeyModifiers::ALT),
        shift: ck.modifiers.contains(crossterm::event::KeyModifiers::SHIFT),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_char_key() {
        let ck = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::empty(),
        );
        let result = convert_crossterm_key(ck).unwrap();
        assert_eq!(result.key, Key::Char('a'));
        assert!(!result.ctrl);
        assert!(!result.alt);
        assert!(!result.shift);
    }

    #[test]
    fn convert_enter_key() {
        let ck = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::empty(),
        );
        let result = convert_crossterm_key(ck).unwrap();
        assert_eq!(result.key, Key::Enter);
    }

    #[test]
    fn convert_ctrl_c() {
        let ck = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('c'),
            crossterm::event::KeyModifiers::CONTROL,
        );
        let result = convert_crossterm_key(ck).unwrap();
        assert_eq!(result.key, Key::Char('c'));
        assert!(result.ctrl);
    }

    #[test]
    fn convert_shift_g() {
        let ck = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('G'),
            crossterm::event::KeyModifiers::SHIFT,
        );
        let result = convert_crossterm_key(ck).unwrap();
        assert_eq!(result.key, Key::Char('G'));
        assert!(result.shift);
    }

    #[test]
    fn convert_unmapped_key_returns_none() {
        let ck = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::F(1),
            crossterm::event::KeyModifiers::empty(),
        );
        assert!(convert_crossterm_key(ck).is_none());
    }

    #[test]
    fn convert_resize_event() {
        let event = crossterm::event::Event::Resize(120, 40);
        let result = convert_crossterm_event(&event).unwrap();
        assert!(matches!(
            result,
            Event::Resize {
                width: 120,
                height: 40
            }
        ));
    }

    #[test]
    fn convert_mouse_event_returns_none() {
        let event = crossterm::event::Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::empty(),
        });
        assert!(convert_crossterm_event(&event).is_none());
    }

    #[test]
    fn all_mapped_keys() {
        let cases: &[(crossterm::event::KeyCode, Key)] = &[
            (crossterm::event::KeyCode::Char('x'), Key::Char('x')),
            (crossterm::event::KeyCode::Enter, Key::Enter),
            (crossterm::event::KeyCode::Esc, Key::Esc),
            (crossterm::event::KeyCode::Tab, Key::Tab),
            (crossterm::event::KeyCode::Backspace, Key::Backspace),
            (crossterm::event::KeyCode::Delete, Key::Delete),
            (crossterm::event::KeyCode::Up, Key::Up),
            (crossterm::event::KeyCode::Down, Key::Down),
            (crossterm::event::KeyCode::Left, Key::Left),
            (crossterm::event::KeyCode::Right, Key::Right),
            (crossterm::event::KeyCode::Home, Key::Home),
            (crossterm::event::KeyCode::End, Key::End),
            (crossterm::event::KeyCode::PageUp, Key::PageUp),
            (crossterm::event::KeyCode::PageDown, Key::PageDown),
        ];
        for (code, expected) in cases {
            let ck =
                crossterm::event::KeyEvent::new(*code, crossterm::event::KeyModifiers::empty());
            let result = convert_crossterm_key(ck).unwrap();
            assert_eq!(&result.key, expected);
        }
    }
}
