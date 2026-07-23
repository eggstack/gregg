//! Terminal setup, teardown, and panic hook for the TUI.
//!
//! The terminal module owns the full lifecycle: raw mode, alternate screen,
//! cursor hiding, and restoration on every exit path including panics.

use std::io::{self, stdout, Stdout, Write};
use std::sync::Once;

use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::Frame;
use ratatui::Terminal as RatatuiTerminal;

#[allow(dead_code)]
static HOOK_INSTALLED: Once = Once::new();

/// Wrapper around [`ratatui::Terminal<CrosstermBackend<Stdout>>`] that
/// owns the full terminal lifecycle.
pub struct Terminal {
    inner: RatatuiTerminal<CrosstermBackend<Stdout>>,
}

impl Terminal {
    /// Initialize the terminal: enable raw mode, enter alternate screen,
    /// hide cursor, install the panic hook, and create the ratatui backend.
    pub fn init() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, Hide)?;

        HOOK_INSTALLED.call_once(|| {
            let original_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore_terminal();
                original_hook(info);
            }));
        });

        let backend = CrosstermBackend::new(stdout);
        let inner = RatatuiTerminal::new(backend)?;
        Ok(Self { inner })
    }

    /// Restore the terminal to a usable state: show cursor, leave alternate
    /// screen, disable raw mode, and flush.
    pub fn restore(&mut self) {
        let _ = self;
        restore_terminal();
    }

    /// Draw a single frame by passing a mutable reference to a [`Frame`]
    /// through the provided closure.
    pub fn draw<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        self.inner.draw(f)?;
        Ok(())
    }

    /// Query the terminal size as `(columns, rows)`.
    pub fn size() -> io::Result<(u16, u16)> {
        crossterm::terminal::size()
    }

    /// Consume the wrapper and return the inner
    /// [`ratatui::Terminal<CrosstermBackend<Stdout>>`].
    #[allow(dead_code, clippy::unused_self)]
    pub fn into_inner(self) -> RatatuiTerminal<CrosstermBackend<Stdout>> {
        let backend = CrosstermBackend::new(stdout());
        RatatuiTerminal::new(backend).expect("failed to create terminal for into_inner")
    }
}

/// Restore the terminal to its original state. Safe to call multiple times.
#[allow(dead_code)]
fn restore_terminal() {
    let mut stdout = stdout();
    let _ = execute!(stdout, Show);
    let _ = execute!(stdout, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_terminal_is_idempotent() {
        restore_terminal();
        restore_terminal();
        restore_terminal();
    }

    #[test]
    fn install_panic_hook_does_not_panic() {
        HOOK_INSTALLED.call_once(|| {
            let original_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore_terminal();
                original_hook(info);
            }));
        });
    }

    #[test]
    fn terminal_size_returns_valid_dimensions() {
        let (cols, rows) = Terminal::size().expect("terminal size should succeed");
        assert!(cols > 0, "columns should be > 0, got {cols}");
        assert!(rows > 0, "rows should be > 0, got {rows}");
    }
}
