//! Shared keyboard selector used by interactive CLI removals.

use std::io::{self, IsTerminal};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

/// Show a selectable list and return the confirmed option index.
///
/// Up/Down and j/k move within the list. Enter confirms; Escape, q, and
/// Ctrl-C cancel without choosing an item.
pub(super) fn select(title: &str, options: &[String]) -> io::Result<Option<usize>> {
    if options.is_empty() {
        return Ok(None);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "interactive removal requires a terminal; provide an endpoint to remove non-interactively",
        ));
    }

    let mut terminal = crate::terminal::Terminal::init()?;
    let result = select_loop(&mut terminal, title, options);
    terminal.restore();
    result
}

fn select_loop(
    terminal: &mut crate::terminal::Terminal,
    title: &str,
    options: &[String],
) -> io::Result<Option<usize>> {
    let mut selected = 0;
    let mut list_state = ListState::default();

    loop {
        list_state.select(Some(selected));
        let list_items: Vec<ListItem<'_>> = options
            .iter()
            .map(|option| ListItem::new(option.as_str()))
            .collect();
        terminal.draw(|frame| {
            let areas = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(frame.area());
            let list = List::new(list_items)
                .block(Block::default().borders(Borders::ALL).title(title))
                .highlight_symbol("> ")
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            frame.render_stateful_widget(list, areas[0], &mut list_state);
            frame.render_widget(
                Paragraph::new("j/k or ↑/↓: move   Enter: remove   Esc/q: cancel"),
                areas[1],
            );
        })?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                selected = move_selection(selected, options.len(), true);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                selected = move_selection(selected, options.len(), false);
            }
            KeyCode::Enter => return Ok(Some(selected)),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(None);
            }
            _ => {}
        }
    }
}

fn move_selection(selected: usize, option_count: usize, down: bool) -> usize {
    if down {
        selected
            .saturating_add(1)
            .min(option_count.saturating_sub(1))
    } else {
        selected.saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::move_selection;

    #[test]
    fn selection_moves_and_stays_within_bounds() {
        assert_eq!(move_selection(0, 3, false), 0);
        assert_eq!(move_selection(0, 3, true), 1);
        assert_eq!(move_selection(1, 3, true), 2);
        assert_eq!(move_selection(2, 3, true), 2);
        assert_eq!(move_selection(2, 3, false), 1);
        assert_eq!(move_selection(0, 0, true), 0);
    }
}
