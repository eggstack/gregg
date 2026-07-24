#![allow(dead_code)]

//! Application state model for the polling engine and TUI.
//!
//! [`AppState`] owns the list of monitored systems, the selection, and the
//! viewport. It is mutated exclusively through [`Action`]s and poll
//! [`PollBatch`]es, making the reducer deterministic and testable.

use std::ops::Range;
use std::time::{Duration, Instant};

use gregg_protocol::StatusSnapshot;

use crate::action::Action;
use crate::config::Config;
use crate::endpoint::Endpoint;
use crate::poller::{PollBatch, PollOutcome};

/// A stable system identifier (UUID v4 string).
pub type SystemId = String;

/// Reachability state for a single system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability {
    /// No poll result received yet.
    Pending,
    /// The most recent poll succeeded.
    Online,
    /// The most recent poll failed.
    Offline,
}

/// Whether the poll scheduler is currently idle or running a generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshStatus {
    /// No poll in progress.
    Idle,
    /// A poll generation is in flight.
    Polling {
        /// The generation number of the in-flight poll.
        generation: u64,
    },
}

/// Per-system mutable state.
#[derive(Debug, Clone)]
pub struct SystemState {
    /// Stable unique identifier matching the config entry.
    pub id: SystemId,
    /// The endpoint used for polling.
    pub endpoint: Endpoint,
    /// Configured display name, if any.
    pub configured_name: Option<String>,
    /// Current reachability.
    pub reachability: Reachability,
    /// Most recent successful snapshot.
    pub latest: Option<StatusSnapshot>,
    /// When the most recent successful poll completed.
    pub last_success_at: Option<Instant>,
    /// When the most recent poll attempt completed (success or failure).
    pub last_attempt_at: Option<Instant>,
    /// Round-trip latency of the most recent successful poll.
    pub latency: Option<Duration>,
    /// The outcome of the most recent failed poll, if any.
    pub last_error: Option<PollOutcome>,
}

/// The top-level application state.
#[derive(Debug)]
pub struct AppState {
    /// Ordered list of all monitored systems.
    pub systems: Vec<SystemState>,
    /// Currently selected system, by stable ID.
    pub selected_id: Option<SystemId>,
    /// The first visible system in the viewport, by stable ID.
    pub viewport_top_id: Option<SystemId>,
    /// Last generation whose results were applied.
    pub last_applied_generation: u64,
    /// Current refresh status.
    pub refresh_status: RefreshStatus,
    /// Terminal dimensions (width, height), if known.
    pub terminal_size: Option<(u16, u16)>,
}

impl AppState {
    /// Create initial state from a configuration.
    ///
    /// All systems start in [`Reachability::Pending`]. The first system
    /// (in display order) is selected if any systems exist.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let systems: Vec<SystemState> = config
            .systems
            .iter()
            .map(|entry| SystemState {
                id: entry.id.clone(),
                endpoint: entry.to_endpoint(),
                configured_name: entry.name.clone(),
                reachability: Reachability::Pending,
                latest: None,
                last_success_at: None,
                last_attempt_at: None,
                latency: None,
                last_error: None,
            })
            .collect();

        let selected_id = systems.first().map(|s| s.id.clone());
        let viewport_top_id = selected_id.clone();

        Self {
            systems,
            selected_id,
            viewport_top_id,
            last_applied_generation: 0,
            refresh_status: RefreshStatus::Idle,
            terminal_size: None,
        }
    }

    /// Apply a poll batch to the state.
    ///
    /// Rejects batches whose generation is less than or equal to the
    /// most recently applied generation. For each result: updates
    /// reachability, latest snapshot, timestamps, latency, and error.
    pub fn apply_batch(&mut self, batch: &PollBatch) {
        if batch.generation <= self.last_applied_generation {
            return;
        }

        for result in &batch.results {
            if let Some(system) = self.systems.iter_mut().find(|s| s.id == result.system_id) {
                match &result.outcome {
                    PollOutcome::Cancelled => {}
                    PollOutcome::Online(snapshot) => {
                        system.reachability = Reachability::Online;
                        system.latest = Some((**snapshot).clone());
                        system.last_success_at = Some(batch.completed_at);
                        system.last_attempt_at = Some(batch.completed_at);
                        system.latency = Some(result.latency);
                        system.last_error = None;
                    }
                    _ => {
                        system.reachability = Reachability::Offline;
                        system.last_attempt_at = Some(batch.completed_at);
                        system.last_error = Some(result.outcome.clone());
                    }
                }
            }
        }

        self.last_applied_generation = batch.generation;
    }

    /// Apply a user action.
    pub fn apply_action(&mut self, action: Action) {
        match action {
            Action::SelectNext => {
                let order = self.display_order();
                self.move_selection(&order, 1);
            }
            Action::SelectPrevious => {
                let order = self.display_order();
                self.move_selection(&order, -1_isize);
            }
            Action::PageDown => {
                let order = self.display_order();
                let page = self.page_size();
                self.move_selection(&order, page);
            }
            Action::PageUp => {
                let order = self.display_order();
                let page = self.page_size();
                self.move_selection(&order, -page);
            }
            Action::SelectFirst => {
                let order = self.display_order();
                self.selected_id = order
                    .first()
                    .and_then(|&i| self.systems.get(i).map(|s| &s.id))
                    .cloned();
            }
            Action::SelectLast => {
                let order = self.display_order();
                self.selected_id = order
                    .last()
                    .and_then(|&i| self.systems.get(i).map(|s| &s.id))
                    .cloned();
            }
            Action::RefreshNow | Action::Quit => {} // Handled by caller.
            Action::ConfigReloaded(config) => self.rebuild_from_config(&config),
            Action::Resize { width, height } => {
                self.terminal_size = Some((width, height));
                ensure_selected_visible(self);
            }
        }
    }

    /// Return the display order: online systems first (in configured
    /// order), then offline/pending systems (in configured order).
    #[must_use]
    pub fn display_order(&self) -> Vec<usize> {
        let mut online = Vec::new();
        let mut offline = Vec::new();

        for (i, system) in self.systems.iter().enumerate() {
            match system.reachability {
                Reachability::Online => online.push(i),
                Reachability::Offline | Reachability::Pending => offline.push(i),
            }
        }

        online.extend(offline);
        online
    }

    /// Rebuild systems from a new config, preserving state for systems
    /// that still exist (matched by ID).
    pub fn rebuild_from_config(&mut self, config: &Config) {
        let old_systems: Vec<SystemState> = self.systems.drain(..).collect();

        self.systems = config
            .systems
            .iter()
            .map(|entry| {
                if let Some(existing) = old_systems.iter().find(|s| s.id == entry.id) {
                    let mut updated = existing.clone();
                    updated.endpoint = entry.to_endpoint();
                    updated.configured_name.clone_from(&entry.name);
                    updated
                } else {
                    SystemState {
                        id: entry.id.clone(),
                        endpoint: entry.to_endpoint(),
                        configured_name: entry.name.clone(),
                        reachability: Reachability::Pending,
                        latest: None,
                        last_success_at: None,
                        last_attempt_at: None,
                        latency: None,
                        last_error: None,
                    }
                }
            })
            .collect();

        // Ensure selection is valid.
        if let Some(ref sel) = self.selected_id {
            if !self.systems.iter().any(|s| &s.id == sel) {
                self.selected_id = self.systems.first().map(|s| s.id.clone());
            }
        } else {
            self.selected_id = self.systems.first().map(|s| s.id.clone());
        }

        // Ensure viewport is valid.
        if let Some(ref top) = self.viewport_top_id {
            if !self.systems.iter().any(|s| &s.id == top) {
                self.viewport_top_id = self.selected_id.clone();
            }
        } else {
            self.viewport_top_id = self.selected_id.clone();
        }
    }

    /// Move selection by a relative offset in display order.
    fn move_selection(&mut self, order: &[usize], offset: isize) {
        if order.is_empty() {
            self.selected_id = None;
            return;
        }

        let current_pos = self
            .selected_id
            .as_ref()
            .and_then(|sel| order.iter().position(|&i| &self.systems[i].id == sel))
            .unwrap_or(0);

        let len = order.len();
        let new_pos = if offset >= 0 {
            current_pos.saturating_add(usize::try_from(offset).unwrap_or(len))
        } else {
            current_pos.saturating_sub(usize::try_from(-offset).unwrap_or(current_pos))
        }
        .min(len - 1);

        self.selected_id = order
            .get(new_pos)
            .and_then(|&i| self.systems.get(i))
            .map(|s| s.id.clone());
    }

    /// Compute the page size (number of systems to skip) based on
    /// terminal height and the current viewport.
    fn page_size(&self) -> isize {
        let height = self.terminal_size.map_or(24, |(_, h)| h).saturating_sub(2); // Reserve rows for headers/footers.

        let order = self.display_order();
        let top_pos = self
            .viewport_top_id
            .as_ref()
            .and_then(|top| order.iter().position(|&i| &self.systems[i].id == top))
            .unwrap_or(0);

        let mut rows = 0_u16;
        let mut count = 0_isize;
        for &idx in order.iter().skip(top_pos) {
            let h = entry_height(&self.systems[idx]);
            if rows + h > height && count > 0 {
                break;
            }
            rows += h;
            count += 1;
        }

        count.max(1)
    }
}

/// Return the row height for a system entry.
///
/// Online entries occupy 4 rows; pending and offline entries occupy 1 row.
#[must_use]
pub fn entry_height(system: &SystemState) -> u16 {
    match system.reachability {
        Reachability::Online => 4,
        Reachability::Pending | Reachability::Offline => 1,
    }
}

/// Compute which systems in display order are visible given a top
/// index, the system states, and available height.
///
/// Online entries take 4 rows; offline entries take 1. Partial entries
/// at the bottom are excluded when possible. If the terminal has fewer
/// than 4 usable rows, returns an empty range.
#[must_use]
pub fn visible_range(
    display_order: &[usize],
    systems: &[SystemState],
    top_index: usize,
    height: u16,
) -> Range<usize> {
    if height < 4 {
        return 0..0;
    }

    let mut rows_used = 0_u16;
    let mut count = 0_usize;

    for &idx in display_order.iter().skip(top_index) {
        if idx >= systems.len() {
            break;
        }
        let h = entry_height(&systems[idx]);

        if rows_used + h > height && count > 0 {
            break;
        }
        rows_used += h;
        count += 1;
    }

    top_index..(top_index + count)
}

/// Adjust `viewport_top_id` so the selected system is visible.
pub fn ensure_selected_visible(state: &mut AppState) {
    let order = state.display_order();
    if order.is_empty() {
        return;
    }

    let (_, height) = state.terminal_size.unwrap_or((80, 24));

    let selected_pos = state
        .selected_id
        .as_ref()
        .and_then(|sel| order.iter().position(|&i| &state.systems[i].id == sel));

    let top_pos = state
        .viewport_top_id
        .as_ref()
        .and_then(|top| order.iter().position(|&i| &state.systems[i].id == top))
        .unwrap_or(0);

    let Some(selected_pos) = selected_pos else {
        return;
    };

    // Compute how many rows the currently visible region takes.
    let usable_height = height.saturating_sub(2); // Reserve for headers/footers.

    // Find which systems fit from top_pos downward.
    let visible = visible_range(&order, &state.systems, top_pos, usable_height);

    if visible.contains(&selected_pos) {
        // Already visible, nothing to do.
        return;
    }

    // If selected is above viewport, scroll up.
    if selected_pos < top_pos {
        state.viewport_top_id = Some(state.systems[order[selected_pos]].id.clone());
        return;
    }

    // If selected is below viewport, scroll down so selected is visible
    // at the top of the viewport.
    if selected_pos >= top_pos {
        state.viewport_top_id = Some(state.systems[order[selected_pos]].id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SystemEntry;
    use gregg_protocol::test_support::LinuxSnapshotBuilder;

    fn test_config_with_ids(ids: &[&str]) -> Config {
        let mut config = Config::default();
        for (i, id) in ids.iter().enumerate() {
            config.systems.push(SystemEntry {
                id: (*id).to_string(),
                host: format!("host{i}.local"),
                port: 11310 + u16::try_from(i).unwrap(),
                name: Some(format!("System {i}")),
            });
        }
        config
    }

    fn make_snapshot() -> StatusSnapshot {
        LinuxSnapshotBuilder::default().build()
    }

    #[test]
    fn from_config_creates_correct_initial_state() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let state = AppState::from_config(&config);

        assert_eq!(state.systems.len(), 3);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
        assert_eq!(state.viewport_top_id.as_deref(), Some("a"));
        assert_eq!(state.last_applied_generation, 0);
        assert_eq!(state.refresh_status, RefreshStatus::Idle);
        assert!(state.terminal_size.is_none());

        for system in &state.systems {
            assert_eq!(system.reachability, Reachability::Pending);
            assert!(system.latest.is_none());
        }
    }

    #[test]
    fn from_config_empty_systems() {
        let config = Config::default();
        let state = AppState::from_config(&config);

        assert!(state.systems.is_empty());
        assert!(state.selected_id.is_none());
        assert!(state.viewport_top_id.is_none());
    }

    #[test]
    fn apply_batch_online_result() {
        let config = test_config_with_ids(&["a", "b"]);
        let mut state = AppState::from_config(&config);
        let snap = make_snapshot();

        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(snap.clone())),
                latency: Duration::from_millis(50),
            }],
        };

        state.apply_batch(&batch);

        assert_eq!(state.systems[0].reachability, Reachability::Online);
        assert!(state.systems[0].latest.is_some());
        assert!(state.systems[0].last_success_at.is_some());
        assert!(state.systems[0].latency.is_some());
        assert!(state.systems[0].last_error.is_none());
        assert_eq!(state.last_applied_generation, 1);
        // System b is still pending.
        assert_eq!(state.systems[1].reachability, Reachability::Pending);
    }

    #[test]
    fn apply_batch_offline_result() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::from_config(&config);

        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::ConnectionRefused,
                latency: Duration::from_millis(10),
            }],
        };

        state.apply_batch(&batch);

        assert_eq!(state.systems[0].reachability, Reachability::Offline);
        assert!(state.systems[0].latest.is_none());
        assert!(state.systems[0].last_attempt_at.is_some());
        assert!(state.systems[0].last_error.is_some());
    }

    #[test]
    fn apply_batch_rejects_old_generation() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::from_config(&config);

        let batch = PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(50),
            }],
        };

        state.apply_batch(&batch);
        assert_eq!(state.last_applied_generation, 2);

        // Older batch should be rejected.
        let old_batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::ConnectionRefused,
                latency: Duration::from_millis(10),
            }],
        };

        state.apply_batch(&old_batch);
        // Generation should not have changed back.
        assert_eq!(state.last_applied_generation, 2);
        // Reachability should still be Online.
        assert_eq!(state.systems[0].reachability, Reachability::Online);
    }

    #[test]
    fn apply_batch_cancelled_no_state_change() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::from_config(&config);

        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Cancelled,
                latency: Duration::from_millis(50),
            }],
        };

        state.apply_batch(&batch);

        // Should still be Pending (not changed by Cancelled).
        assert_eq!(state.systems[0].reachability, Reachability::Pending);
    }

    #[test]
    fn display_order_online_first() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);

        // Make b online.
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "b".into(),
                endpoint: state.systems[1].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(50),
            }],
        };
        state.apply_batch(&batch);

        let order = state.display_order();
        // b is online, should be first. a and c are pending, should follow.
        assert_eq!(order.len(), 3);
        assert_eq!(state.systems[order[0]].id, "b");
        // a and c should maintain configured order.
        let remaining: Vec<&str> = order[1..]
            .iter()
            .map(|&i| state.systems[i].id.as_str())
            .collect();
        assert_eq!(remaining, vec!["a", "c"]);
    }

    #[test]
    fn display_order_preserves_configured_order() {
        let config = test_config_with_ids(&["a", "b", "c", "d"]);
        let mut state = AppState::from_config(&config);

        // Make c and a online.
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![
                crate::poller::PollResult {
                    system_id: "c".into(),
                    endpoint: state.systems[2].endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(50),
                },
                crate::poller::PollResult {
                    system_id: "a".into(),
                    endpoint: state.systems[0].endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(50),
                },
            ],
        };
        state.apply_batch(&batch);

        let order = state.display_order();
        // Online: a (index 0), c (index 2) in configured order.
        assert_eq!(state.systems[order[0]].id, "a");
        assert_eq!(state.systems[order[1]].id, "c");
        // Offline: b, d in configured order.
        assert_eq!(state.systems[order[2]].id, "b");
        assert_eq!(state.systems[order[3]].id, "d");
    }

    #[test]
    fn select_next_moves_forward() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);

        assert_eq!(state.selected_id.as_deref(), Some("a"));

        state.apply_action(Action::SelectNext);
        assert_eq!(state.selected_id.as_deref(), Some("b"));

        state.apply_action(Action::SelectNext);
        assert_eq!(state.selected_id.as_deref(), Some("c"));

        // Should clamp at the end.
        state.apply_action(Action::SelectNext);
        assert_eq!(state.selected_id.as_deref(), Some("c"));
    }

    #[test]
    fn select_previous_moves_backward() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);

        state.apply_action(Action::SelectNext);
        state.apply_action(Action::SelectNext);
        assert_eq!(state.selected_id.as_deref(), Some("c"));

        state.apply_action(Action::SelectPrevious);
        assert_eq!(state.selected_id.as_deref(), Some("b"));

        state.apply_action(Action::SelectPrevious);
        assert_eq!(state.selected_id.as_deref(), Some("a"));

        // Should clamp at the beginning.
        state.apply_action(Action::SelectPrevious);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
    }

    #[test]
    fn select_first_and_last() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);

        state.apply_action(Action::SelectLast);
        assert_eq!(state.selected_id.as_deref(), Some("c"));

        state.apply_action(Action::SelectFirst);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
    }

    #[test]
    fn page_down_and_up() {
        let config = test_config_with_ids(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        let mut state = AppState::from_config(&config);
        state.terminal_size = Some((80, 20));

        state.apply_action(Action::PageDown);
        // Page size should be > 1, so selection should move.
        let after_page_down = state.selected_id.clone();
        assert_ne!(after_page_down.as_deref(), Some("a"));

        state.apply_action(Action::PageUp);
        // Should move back toward the beginning.
        let after_page_up = state.selected_id.clone();
        assert_eq!(after_page_up.as_deref(), Some("a"));
    }

    #[test]
    fn selection_preserved_across_reorder() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);

        // Select b.
        state.apply_action(Action::SelectNext);
        assert_eq!(state.selected_id.as_deref(), Some("b"));

        // Make a online (changes display order but b is still selected).
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(50),
            }],
        };
        state.apply_batch(&batch);

        assert_eq!(state.selected_id.as_deref(), Some("b"));
    }

    #[test]
    fn selection_falls_back_when_system_removed() {
        let config = test_config_with_ids(&["a", "b"]);
        let mut state = AppState::from_config(&config);

        state.apply_action(Action::SelectNext);
        assert_eq!(state.selected_id.as_deref(), Some("b"));

        // Rebuild with only "a".
        let mut new_config = Config::default();
        new_config.systems.push(SystemEntry {
            id: "a".into(),
            host: "host0.local".into(),
            port: 11310,
            name: None,
        });

        state.rebuild_from_config(&new_config);
        // b is gone, selection should fall back to a.
        assert_eq!(state.selected_id.as_deref(), Some("a"));
    }

    #[test]
    fn rebuild_from_config_preserves_existing_state() {
        let config = test_config_with_ids(&["a", "b"]);
        let mut state = AppState::from_config(&config);

        // Give a a snapshot.
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(50),
            }],
        };
        state.apply_batch(&batch);
        assert_eq!(state.systems[0].reachability, Reachability::Online);

        // Rebuild with same config.
        state.rebuild_from_config(&config);

        // State should be preserved.
        assert_eq!(state.systems.len(), 2);
        assert_eq!(state.systems[0].reachability, Reachability::Online);
        assert!(state.systems[0].latest.is_some());
        assert_eq!(state.systems[0].id, "a");
    }

    #[test]
    fn rebuild_from_config_adds_new_system() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::from_config(&config);

        let mut new_config = test_config_with_ids(&["a", "b"]);
        // Use same ID for a so it's recognized.
        new_config.systems[0].id = "a".into();

        state.rebuild_from_config(&new_config);
        assert_eq!(state.systems.len(), 2);
        // New system b should be pending.
        assert_eq!(state.systems[1].reachability, Reachability::Pending);
    }

    #[test]
    fn entry_height_online_is_four() {
        let mut system = SystemState {
            id: "test".into(),
            endpoint: Endpoint::new("host".into(), 11310, None),
            configured_name: None,
            reachability: Reachability::Online,
            latest: None,
            last_success_at: None,
            last_attempt_at: None,
            latency: None,
            last_error: None,
        };
        assert_eq!(entry_height(&system), 4);

        system.reachability = Reachability::Pending;
        assert_eq!(entry_height(&system), 1);

        system.reachability = Reachability::Offline;
        assert_eq!(entry_height(&system), 1);
    }

    #[test]
    fn visible_range_handles_mixed_heights() {
        let config = test_config_with_ids(&["a", "b", "c", "d", "e"]);
        let state = AppState::from_config(&config);
        let order = state.display_order();
        let range = visible_range(&order, &state.systems, 0, 20);
        // Should include some entries.
        assert!(!range.is_empty());
    }

    #[test]
    fn visible_range_small_terminal() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let state = AppState::from_config(&config);
        let order = state.display_order();
        let range = visible_range(&order, &state.systems, 0, 3);
        // Terminal too small for even one online entry.
        assert!(range.is_empty());
    }

    #[test]
    fn ensure_selected_visible_adjusts_viewport() {
        let config = test_config_with_ids(&["a", "b", "c", "d", "e"]);
        let mut state = AppState::from_config(&config);
        state.terminal_size = Some((80, 6)); // Very small: 4 usable rows

        // Select the last system.
        state.apply_action(Action::SelectLast);
        assert_eq!(state.selected_id.as_deref(), Some("e"));

        // Ensure selected is visible.
        ensure_selected_visible(&mut state);

        // The viewport should have been adjusted so e is visible.
        let order = state.display_order();
        let top_pos = state
            .viewport_top_id
            .as_ref()
            .and_then(|top| order.iter().position(|&i| &state.systems[i].id == top));
        let selected_pos = order
            .iter()
            .position(|&i| &state.systems[i].id == state.selected_id.as_ref().unwrap());
        assert!(top_pos.is_some());
        assert!(selected_pos.is_some());
        assert!(selected_pos.unwrap() >= top_pos.unwrap());
    }

    #[test]
    fn config_reloaded_action() {
        let config = test_config_with_ids(&["a", "b"]);
        let mut state = AppState::from_config(&config);

        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(50),
            }],
        };
        state.apply_batch(&batch);

        // Reload with new config.
        let mut new_config = test_config_with_ids(&["a", "b", "c"]);
        new_config.systems[0].id = "a".into();
        new_config.systems[1].id = "b".into();
        new_config.systems[2].id = "c".into();

        state.apply_action(Action::ConfigReloaded(new_config));

        assert_eq!(state.systems.len(), 3);
        // a's state should be preserved.
        assert_eq!(state.systems[0].reachability, Reachability::Online);
    }

    #[test]
    fn resize_updates_terminal_size() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::from_config(&config);

        state.apply_action(Action::Resize {
            width: 120,
            height: 40,
        });

        assert_eq!(state.terminal_size, Some((120, 40)));
    }

    #[test]
    fn empty_config_no_selection() {
        let config = Config::default();
        let mut state = AppState::from_config(&config);

        state.apply_action(Action::SelectNext);
        assert!(state.selected_id.is_none());

        state.apply_action(Action::SelectPrevious);
        assert!(state.selected_id.is_none());

        state.apply_action(Action::SelectFirst);
        assert!(state.selected_id.is_none());

        state.apply_action(Action::SelectLast);
        assert!(state.selected_id.is_none());
    }

    #[test]
    fn multiple_systems_online_offline_mixed_display_order() {
        let config = test_config_with_ids(&["a", "b", "c", "d", "e"]);
        let mut state = AppState::from_config(&config);

        // Make a, c, e online.
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![
                crate::poller::PollResult {
                    system_id: "a".into(),
                    endpoint: state.systems[0].endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(50),
                },
                crate::poller::PollResult {
                    system_id: "c".into(),
                    endpoint: state.systems[2].endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(50),
                },
                crate::poller::PollResult {
                    system_id: "e".into(),
                    endpoint: state.systems[4].endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(50),
                },
            ],
        };
        state.apply_batch(&batch);

        let order = state.display_order();
        assert_eq!(order.len(), 5);
        // Online first: a, c, e (in configured order).
        assert_eq!(state.systems[order[0]].id, "a");
        assert_eq!(state.systems[order[1]].id, "c");
        assert_eq!(state.systems[order[2]].id, "e");
        // Offline: b, d.
        assert_eq!(state.systems[order[3]].id, "b");
        assert_eq!(state.systems[order[4]].id, "d");
    }
}
