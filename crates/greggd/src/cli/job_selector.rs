//! Minimal terminal selector for choosing one configured scheduled job.

use std::io::{self, IsTerminal, Write};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

use crate::config::ScheduledJobConfig;

/// Show configured jobs and return the selected index, or None on cancel.
pub(super) fn select(jobs: &[ScheduledJobConfig]) -> io::Result<Option<usize>> {
    if jobs.is_empty() {
        return Ok(None);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "interactive job removal requires a terminal; provide a job name to remove non-interactively",
        ));
    }

    let mut guard = TerminalGuard::enter()?;
    let selection = select_loop(jobs);
    let restoration = guard.restore();
    match (selection, restoration) {
        (Ok(selection), Ok(())) => Ok(selection),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(restore_error)) => Err(io::Error::other(format!(
            "selector failed: {error}; terminal restoration also failed: {restore_error}"
        ))),
    }
}

fn select_loop(jobs: &[ScheduledJobConfig]) -> io::Result<Option<usize>> {
    let mut selected = 0;
    let mut stdout = io::stdout();
    loop {
        render(jobs, selected, &mut stdout)?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                selected = move_selection(selected, jobs.len(), true);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                selected = move_selection(selected, jobs.len(), false);
            }
            KeyCode::Enter => return Ok(Some(selected)),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(None);
            }
            _ => {}
        }
    }
}

fn render(jobs: &[ScheduledJobConfig], selected: usize, stdout: &mut impl Write) -> io::Result<()> {
    let (_, height) = terminal::size()?;
    let visible_rows = usize::from(height.saturating_sub(4)).max(1);
    let (start, end) = visible_range(selected, jobs.len(), visible_rows);
    execute!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;
    writeln!(stdout, "Remove a scheduled job ({})", jobs.len())?;
    for (index, job) in jobs.iter().enumerate().take(end).skip(start) {
        let marker = if index == selected { ">" } else { " " };
        writeln!(stdout, "{marker} {}  [{}]", job.name, job.schedule)?;
    }
    writeln!(stdout, "Job {} of {}", selected + 1, jobs.len())?;
    writeln!(
        stdout,
        "j/k or ↑/↓: move   Enter: remove   Esc/q/Ctrl-C: cancel"
    )?;
    stdout.flush()
}

fn visible_range(selected: usize, count: usize, visible_rows: usize) -> (usize, usize) {
    let rows = visible_rows.max(1).min(count);
    let start = selected
        .saturating_sub(rows / 2)
        .min(count.saturating_sub(rows));
    (start, start + rows)
}

fn move_selection(selected: usize, count: usize, down: bool) -> usize {
    if down {
        selected.saturating_add(1).min(count.saturating_sub(1))
    } else {
        selected.saturating_sub(1)
    }
}

struct TerminalGuard {
    raw_mode: bool,
    alternate_screen: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut guard = Self {
            raw_mode: true,
            alternate_screen: true,
        };
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = guard.restore();
            return Err(error);
        }
        Ok(guard)
    }

    fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        if self.alternate_screen {
            match execute!(io::stdout(), Show, LeaveAlternateScreen) {
                Ok(()) => self.alternate_screen = false,
                Err(error) => first_error = Some(error),
            }
        }
        if self.raw_mode {
            match terminal::disable_raw_mode() {
                Ok(()) => self.raw_mode = false,
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::{move_selection, visible_range};

    #[test]
    fn selector_movement_stays_within_bounds() {
        assert_eq!(move_selection(0, 3, false), 0);
        assert_eq!(move_selection(0, 3, true), 1);
        assert_eq!(move_selection(2, 3, true), 2);
        assert_eq!(move_selection(2, 3, false), 1);
        assert_eq!(move_selection(0, 0, true), 0);
    }

    #[test]
    fn selector_windows_around_selection_and_clamps_at_ends() {
        assert_eq!(visible_range(0, 10, 4), (0, 4));
        assert_eq!(visible_range(5, 10, 4), (3, 7));
        assert_eq!(visible_range(9, 10, 4), (6, 10));
        assert_eq!(visible_range(3, 3, 8), (0, 3));
    }
}
