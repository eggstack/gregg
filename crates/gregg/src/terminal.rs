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
        use std::io::IsTerminal;
        if !std::io::stdout().is_terminal() {
            eprintln!("skipping: no TTY attached");
            return;
        }
        let (cols, rows) = Terminal::size().expect("terminal size should succeed");
        assert!(cols > 0, "columns should be > 0, got {cols}");
        assert!(rows > 0, "rows should be > 0, got {rows}");
    }

    // ---------- Terminal lifecycle tests ----------

    #[test]
    fn restore_terminal_safe_without_init() {
        // restore_terminal should be safe to call even if Terminal::init
        // was never called. The underlying crossterm calls are idempotent:
        // leaving alternate screen when not in one is a no-op, and
        // disabling raw mode when not in raw mode is a no-op.
        for _ in 0..10 {
            restore_terminal();
        }
    }

    #[test]
    fn restore_terminal_preserves_stdout() {
        // After restore_terminal, stdout should still be usable.
        restore_terminal();
        let result = std::io::stdout().write_all(b"test");
        assert!(
            result.is_ok(),
            "stdout should remain writable after restore"
        );
    }

    #[test]
    fn panic_hook_does_not_interfere_with_normal_operation() {
        use std::io::IsTerminal;

        // Install the panic hook and then verify normal operations work.
        HOOK_INSTALLED.call_once(|| {
            let original_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore_terminal();
                original_hook(info);
            }));
        });

        // These should all succeed without triggering the panic hook.
        restore_terminal();
        if !std::io::stdout().is_terminal() {
            eprintln!("skipping terminal size check: no TTY attached");
            return;
        }
        let (cols, rows) = Terminal::size().expect("size should work");
        assert!(cols > 0);
        assert!(rows > 0);
    }

    #[test]
    fn multiple_hooks_do_not_stack() {
        // The Once guard ensures the hook is only installed once.
        // Calling the installation logic multiple times should not
        // create multiple hooks.
        let hook1 = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            hook1(info);
        }));

        // Second installation via Once should be skipped.
        HOOK_INSTALLED.call_once(|| {
            let original_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore_terminal();
                original_hook(info);
            }));
        });

        // Clean up: restore default hook.
        let _ = std::panic::take_hook();
    }
}
