//! Application state model for the polling engine and TUI.
//!
//! [`AppState`] owns the list of monitored systems, the selection, and the
//! viewport. It is mutated exclusively through [`Action`]s and poll
//! [`PollBatch`]es, making the reducer deterministic and testable.

use std::ops::Range;
use std::time::{Duration, Instant};

use crate::action::Action;
use crate::config::Config;
use crate::eggpool::{EggpoolFetchOutcome, EggpoolPeriod, EggpoolResult, EggpoolSummary};
use crate::endpoint::Endpoint;
use crate::normalized::NormalizedSnapshot;
use crate::poller::{PollBatch, PollOutcome};

/// A stable system identifier (UUID v4 string).
pub type SystemId = String;

/// Reachability state for a single system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// The TUI presentation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemViewMode {
    /// The detailed, one-block-per-system view.
    Normal,
    /// The one-row-per-system fleet view.
    Condensed,
}

/// The two fixed top-level panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// The configured system fleet.
    Systems,
    /// The optional `EggPool` summary.
    Eggpool,
}

/// Whether the `EggPool` pane is waiting for or displaying a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EggpoolStatus {
    /// No request is currently in flight.
    Idle,
    /// A request has been requested and is being dispatched.
    Refreshing,
    /// The last command could not be queued; the worker was busy.
    Busy,
    /// The local worker is unavailable; no request can be dispatched.
    WorkerUnavailable,
}

/// Reducer-owned transient state for the optional `EggPool` pane.
#[derive(Debug, Clone)]
pub struct EggpoolState {
    /// The configured source displayed by the pane.
    pub endpoint: crate::config::EggpoolEntry,
    /// Currently selected rolling window.
    pub period: EggpoolPeriod,
    /// Latest desired request identity.
    pub request_generation: u64,
    /// Current request status.
    pub status: EggpoolStatus,
    /// Last successful summary for the selected period.
    pub summary: Option<EggpoolSummary>,
    /// Completion time of the last successful request.
    pub last_success_at: Option<Instant>,
    /// Completion time of the last request attempt.
    pub last_attempt_at: Option<Instant>,
    /// Most recent non-cancelled failure.
    pub last_error: Option<EggpoolFetchOutcome>,
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
    /// Most recent successful snapshot (normalized from v1 or v2).
    pub latest: Option<NormalizedSnapshot>,
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
    /// Diagnostic from the most recent rejected Systems config reload.
    pub config_reload_error: Option<String>,
    /// Terminal dimensions (width, height), if known.
    pub terminal_size: Option<(u16, u16)>,
    /// Currently active top-level pane.
    pub active_pane: Pane,
    /// Current Systems presentation mode.
    pub system_view_mode: SystemViewMode,
    /// Whether the selected online system's drives are expanded.
    pub drives_expanded: bool,
    /// Plan 087: whether the logical selection is currently being
    /// visually highlighted with `Modifier::REVERSED`. Independent of
    /// `selected_id`; cleared by the event-loop timer (about ten
    /// seconds of inactivity) and on pane changes away from Systems.
    /// Logical selection itself remains available for keyboard actions
    /// (`e` and friends) when this flag is `false`.
    pub selection_highlight_active: bool,
    /// Optional `EggPool` pane state.
    pub eggpool: Option<EggpoolState>,
}

impl AppState {
    /// Create initial state from a configuration.
    ///
    /// All systems start in [`Reachability::Pending`]. The first system
    /// (in display order) is selected if any systems exist.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let systems: Vec<SystemState> = config.systems.iter().map(system_from_entry).collect();

        let selected_id = systems.first().map(|s| s.id.clone());
        let viewport_top_id = selected_id.clone();

        let eggpool = config.eggpool.clone().map(|endpoint| EggpoolState {
            endpoint,
            period: EggpoolPeriod::Hour,
            request_generation: 0,
            status: EggpoolStatus::Idle,
            summary: None,
            last_success_at: None,
            last_attempt_at: None,
            last_error: None,
        });
        Self {
            systems,
            selected_id,
            viewport_top_id,
            last_applied_generation: 0,
            refresh_status: RefreshStatus::Idle,
            config_reload_error: None,
            terminal_size: None,
            active_pane: if config.systems.is_empty() && eggpool.is_some() {
                Pane::Eggpool
            } else {
                Pane::Systems
            },
            system_view_mode: SystemViewMode::Normal,
            drives_expanded: false,
            selection_highlight_active: false,
            eggpool,
        }
    }

    /// Reconcile the configured system endpoint list while retaining safe
    /// state for unchanged stable IDs.
    pub fn reconcile_systems(&mut self, config: &Config) {
        let old_systems = std::mem::take(&mut self.systems);
        let old_selected = self.selected_id.clone();
        let old_by_id = old_systems
            .into_iter()
            .map(|system| (system.id.clone(), system))
            .collect::<std::collections::HashMap<_, _>>();

        self.systems = config
            .systems
            .iter()
            .map(|entry| {
                let Some(mut old) = old_by_id.get(&entry.id).cloned() else {
                    return system_from_entry(entry);
                };

                let endpoint = entry.to_endpoint();
                if old.endpoint.host.eq_ignore_ascii_case(&endpoint.host)
                    && old.endpoint.port == endpoint.port
                {
                    old.endpoint = endpoint;
                    old.configured_name.clone_from(&entry.name);
                    old
                } else {
                    system_from_entry(entry)
                }
            })
            .collect();

        self.selected_id = old_selected
            .filter(|id| self.systems.iter().any(|system| &system.id == id))
            .or_else(|| self.systems.first().map(|system| system.id.clone()));
        self.viewport_top_id = self
            .viewport_top_id
            .take()
            .filter(|id| self.systems.iter().any(|system| &system.id == id))
            .or_else(|| self.selected_id.clone());
        ensure_selected_visible(self);
        if self.systems.is_empty() {
            self.selected_id = None;
            self.viewport_top_id = None;
        }
    }

    /// Record a rejected Systems config reload for the renderer.
    pub fn set_config_reload_error(&mut self, error: String) {
        self.config_reload_error = Some(error);
    }

    /// Clear the rejected Systems config reload diagnostic after a success.
    pub fn clear_config_reload_error(&mut self) {
        self.config_reload_error = None;
    }

    /// Apply a poll batch to the state.
    ///
    /// Rejects batches whose generation is less than or equal to the
    /// most recently applied generation, except for the scheduler's single
    /// `u64::MAX` to `1` wrap. For each result: updates reachability, latest
    /// snapshot, timestamps, latency, and error.
    pub fn apply_batch(&mut self, batch: &PollBatch) {
        // The scheduler advances by exactly one and wraps only MAX -> 1;
        // do not accept a skipped-generation wrap as a fresh batch.
        let wrapped_generation = self.last_applied_generation == u64::MAX && batch.generation == 1;
        if batch.generation <= self.last_applied_generation && !wrapped_generation {
            return;
        }

        let was_initialized = self.last_applied_generation == 0;

        for result in &batch.results {
            if let Some(system) = self.systems.iter_mut().find(|s| s.id == result.system_id) {
                // A stable ID may be retained while its configured target
                // changes. Results from the superseded target are stale even
                // when their scheduler generation is otherwise current.
                if system.endpoint.host != result.endpoint.host
                    || system.endpoint.port != result.endpoint.port
                {
                    continue;
                }
                match &result.outcome {
                    PollOutcome::Cancelled => {}
                    PollOutcome::Online(snapshot) => {
                        system.reachability = Reachability::Online;
                        system.latest = Some(NormalizedSnapshot::from_v1(snapshot));
                        system.last_success_at = Some(batch.completed_at);
                        system.last_attempt_at = Some(batch.completed_at);
                        system.latency = Some(result.latency);
                        system.last_error = None;
                    }
                    PollOutcome::OnlineV2(snapshot) => {
                        system.reachability = Reachability::Online;
                        system.latest = Some(NormalizedSnapshot::from_v2_payload(snapshot));
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

        let order = self.display_order();

        // The first accepted poll batch establishes the
        // reachability-sorted display order for a fresh TUI. Pinning
        // selection and viewport top to that order prevents an offline
        // first-configured system from dragging the viewport below any
        // online entries that came back first. Later batches must
        // preserve ordinary selection/scroll semantics.
        if was_initialized {
            if let Some(&first_index) = order.first() {
                if let Some(first_system) = self.systems.get(first_index) {
                    let id = first_system.id.clone();
                    self.selected_id = Some(id.clone());
                    self.viewport_top_id = Some(id);
                }
            }
        }

        ensure_selected_visible_with_order(self, &order);
    }

    /// Apply a user action.
    #[allow(clippy::match_same_arms)]
    pub fn apply_action(&mut self, action: Action) {
        match action {
            Action::MoveDown => {
                if self.active_pane == Pane::Eggpool {
                    self.move_eggpool_period(true);
                } else {
                    let order = self.display_order();
                    self.move_selection(&order, 1);
                    self.selection_highlight_active = true;
                    ensure_selected_visible_with_order(self, &order);
                    return;
                }
            }
            Action::MoveUp => {
                if self.active_pane == Pane::Eggpool {
                    self.move_eggpool_period(false);
                } else {
                    let order = self.display_order();
                    self.move_selection(&order, -1_isize);
                    self.selection_highlight_active = true;
                    ensure_selected_visible_with_order(self, &order);
                    return;
                }
            }
            Action::PageDown if self.active_pane == Pane::Systems => {
                let order = self.display_order();
                let page = self.page_size(&order);
                self.move_selection(&order, page);
                self.selection_highlight_active = true;
                ensure_selected_visible_with_order(self, &order);
                return;
            }
            Action::PageUp if self.active_pane == Pane::Systems => {
                let order = self.display_order();
                let page = self.page_size(&order);
                self.move_selection(&order, -page);
                self.selection_highlight_active = true;
                ensure_selected_visible_with_order(self, &order);
                return;
            }
            Action::SelectFirst if self.active_pane == Pane::Systems => {
                let order = self.display_order();
                self.selected_id = order
                    .first()
                    .and_then(|&i| self.systems.get(i).map(|s| &s.id))
                    .cloned();
                self.selection_highlight_active = true;
                ensure_selected_visible_with_order(self, &order);
                return;
            }
            Action::SelectLast if self.active_pane == Pane::Systems => {
                let order = self.display_order();
                self.selected_id = order
                    .last()
                    .and_then(|&i| self.systems.get(i).map(|s| &s.id))
                    .cloned();
                self.selection_highlight_active = true;
                ensure_selected_visible_with_order(self, &order);
                return;
            }
            Action::PreviousPane => self.cycle_pane(false),
            Action::NextPane => self.cycle_pane(true),
            Action::ClearSelectionHighlight => {
                self.selection_highlight_active = false;
            }
            Action::ToggleSystemView if self.active_pane == Pane::Systems => {
                self.system_view_mode = match self.system_view_mode {
                    SystemViewMode::Normal => SystemViewMode::Condensed,
                    SystemViewMode::Condensed => SystemViewMode::Normal,
                };
            }
            // These arms change nothing that affects selection visibility,
            // so skip the viewport fix-up below.
            Action::PageDown
            | Action::PageUp
            | Action::SelectFirst
            | Action::SelectLast
            | Action::RefreshNow
            | Action::Quit => return,
            // Note: `ToggleSystemView` on the Systems pane deliberately
            // falls through because view mode changes entry heights.
            Action::ToggleSystemView | Action::ToggleDrives
                if self.active_pane == Pane::Eggpool =>
            {
                return
            }
            Action::ToggleSystemView => return,
            Action::ToggleDrives => {
                self.drives_expanded = !self.drives_expanded;
            }
            Action::Resize { width, height } => {
                self.terminal_size = Some((width, height));
            }
        }
        ensure_selected_visible(self);
    }

    /// Return the display order: online systems first (in configured
    /// order), then offline/pending systems (in configured order).
    #[must_use]
    pub fn display_order(&self) -> Vec<usize> {
        // One allocation: online indices are appended first, then a second
        // pass appends the offline/pending indices, preserving configured
        // order within each group.
        let mut order = Vec::with_capacity(self.systems.len());
        for (i, system) in self.systems.iter().enumerate() {
            if matches!(system.reachability, Reachability::Online) {
                order.push(i);
            }
        }
        for (i, system) in self.systems.iter().enumerate() {
            if matches!(
                system.reachability,
                Reachability::Offline | Reachability::Pending
            ) {
                order.push(i);
            }
        }
        order
    }

    /// Apply one `EggPool` result if it belongs to the current request and period.
    pub fn apply_eggpool_result(&mut self, result: &EggpoolResult) {
        let Some(eggpool) = self.eggpool.as_mut() else {
            return;
        };
        if result.generation != eggpool.request_generation || result.period != eggpool.period {
            return;
        }
        if !matches!(result.outcome, EggpoolFetchOutcome::Cancelled) {
            eggpool.status = EggpoolStatus::Idle;
            eggpool.last_attempt_at = Some(result.completed_at);
            match &result.outcome {
                EggpoolFetchOutcome::Online(summary) => {
                    eggpool.summary = Some(summary.clone());
                    eggpool.last_success_at = Some(result.completed_at);
                    eggpool.last_error = None;
                }
                error => eggpool.last_error = Some(error.clone()),
            }
        }
    }

    /// Mark an `EggPool` activation or manual refresh as a new request.
    pub fn begin_eggpool_request(&mut self) -> Option<(EggpoolPeriod, u64)> {
        let eggpool = self.eggpool.as_mut()?;
        eggpool.request_generation = eggpool.request_generation.saturating_add(1);
        eggpool.status = EggpoolStatus::Refreshing;
        Some((eggpool.period, eggpool.request_generation))
    }

    /// Mark the local worker as unavailable without exposing a channel error.
    pub fn mark_eggpool_worker_unavailable(&mut self) {
        if let Some(eggpool) = self.eggpool.as_mut() {
            eggpool.status = EggpoolStatus::WorkerUnavailable;
        }
    }

    /// Mark the pane as busy because the worker's command queue was full.
    pub fn mark_eggpool_busy(&mut self) {
        if let Some(eggpool) = self.eggpool.as_mut() {
            eggpool.status = EggpoolStatus::Busy;
        }
    }

    /// Return the current `EggPool` request identity without changing state.
    #[must_use]
    pub fn eggpool_request(&self) -> Option<(EggpoolPeriod, u64)> {
        self.eggpool
            .as_ref()
            .map(|eggpool| (eggpool.period, eggpool.request_generation))
    }

    fn move_eggpool_period(&mut self, longer: bool) {
        let Some(eggpool) = self.eggpool.as_mut() else {
            return;
        };
        let next = if longer {
            eggpool.period.longer()
        } else {
            eggpool.period.shorter()
        };
        if next == eggpool.period {
            return;
        }
        eggpool.period = next;
        eggpool.request_generation = eggpool.request_generation.saturating_add(1);
        eggpool.status = EggpoolStatus::Refreshing;
        eggpool.summary = None;
        eggpool.last_error = None;
    }

    fn cycle_pane(&mut self, next: bool) {
        let before = self.active_pane;
        match (
            self.active_pane,
            self.systems.is_empty(),
            self.eggpool.is_some(),
            next,
        ) {
            (Pane::Systems, false, true, _) => self.active_pane = Pane::Eggpool,
            (Pane::Eggpool, _, true, _) if !self.systems.is_empty() => {
                self.active_pane = Pane::Systems;
            }
            _ => {}
        }
        // Plan 087: leaving the Systems pane must drop the visual
        // selection highlight immediately so a stale reversed row
        // does not reappear when the operator comes back. Re-entering
        // Systems does not activate the highlight on its own.
        if before == Pane::Systems && self.active_pane != Pane::Systems {
            self.selection_highlight_active = false;
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
        let magnitude = offset.unsigned_abs();
        let new_pos = if offset >= 0 {
            current_pos.saturating_add(magnitude)
        } else {
            current_pos.saturating_sub(magnitude)
        }
        .min(len - 1);

        self.selected_id = order
            .get(new_pos)
            .and_then(|&i| self.systems.get(i))
            .map(|s| s.id.clone());
    }

    /// Compute the page size (number of systems to skip) based on
    /// terminal height and the current viewport.
    ///
    /// Returns zero when the viewport cannot render even one full
    /// entry, so page movement never lands selection on entries that
    /// cannot be displayed.
    fn page_size(&self, order: &[usize]) -> isize {
        let height = self
            .terminal_size
            .map_or(24, |(_, h)| h)
            .saturating_sub(view_header_height(self.system_view_mode));

        let top_pos = self
            .viewport_top_id
            .as_ref()
            .and_then(|top| order.iter().position(|&i| &self.systems[i].id == top))
            .unwrap_or(0);

        let mut rows = 0_u16;
        let mut count = 0_isize;
        for &idx in order.iter().skip(top_pos) {
            let h = entry_height(self, idx);
            if rows + h > height {
                if count == 0 {
                    return 0;
                }
                break;
            }
            rows += h;
            count += 1;
        }

        count.max(1)
    }
}

fn system_from_entry(entry: &crate::config::SystemEntry) -> SystemState {
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

/// Return the full row height for a system entry in the current view.
#[must_use]
pub fn entry_height(state: &AppState, system_index: usize) -> u16 {
    let Some(system) = state.systems.get(system_index) else {
        return 0;
    };
    match (state.system_view_mode, system.reachability) {
        (SystemViewMode::Condensed, _) => {
            if state.drives_expanded
                && state.selected_id.as_deref() == Some(system.id.as_str())
                && system.reachability == Reachability::Online
            {
                1_u16.saturating_add(valid_drive_count(system))
            } else {
                1
            }
        }
        (SystemViewMode::Normal, Reachability::Pending | Reachability::Offline) => 1,
        (SystemViewMode::Normal, Reachability::Online) => {
            let details = if state.drives_expanded
                && state.selected_id.as_deref() == Some(system.id.as_str())
            {
                system
                    .latest
                    .as_ref()
                    .and_then(|snapshot| snapshot.drives.as_ref())
                    .map_or(0, |drives| valid_drive_count_from_slice(drives))
            } else {
                0
            };
            5_u16.saturating_add(details)
        }
    }
}

/// Compute which systems in display order are visible given a top
/// index, the system states, and available height.
///
/// Online entries take five base rows, with optional selected-system drive
/// rows; offline and pending entries take one row. A first entry is retained
/// even when its full dynamic height is taller than the viewport so the caller
/// can clip only detail rows while preserving its complete base block.
#[must_use]
pub fn visible_range(
    display_order: &[usize],
    state: &AppState,
    top_index: usize,
    height: u16,
) -> Range<usize> {
    if height == 0 {
        return 0..0;
    }

    let mut rows_used = 0_u16;
    let mut count = 0_usize;

    for &idx in display_order.iter().skip(top_index) {
        if idx >= state.systems.len() {
            break;
        }
        let h = entry_height(state, idx);

        if count == 0 && height < minimum_render_height(state, idx) {
            return top_index..top_index;
        }

        if rows_used + h > height && count > 0 {
            break;
        }
        rows_used += h;
        count += 1;
    }

    top_index..(top_index + count)
}

fn minimum_render_height(state: &AppState, system_index: usize) -> u16 {
    match state
        .systems
        .get(system_index)
        .map(|system| (state.system_view_mode, system.reachability))
    {
        Some((SystemViewMode::Normal, Reachability::Online)) => 5,
        Some(_) => 1,
        None => 0,
    }
}

/// Adjust `viewport_top_id` so the selected system is visible.
pub fn ensure_selected_visible(state: &mut AppState) {
    let order = state.display_order();
    ensure_selected_visible_with_order(state, &order);
}

/// [`ensure_selected_visible`] with a precomputed display order, so
/// callers already holding one avoid rebuilding it.
fn ensure_selected_visible_with_order(state: &mut AppState, order: &[usize]) {
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

    // The renderer uses the complete frame as its viewport.
    let usable_height = height.saturating_sub(view_header_height(state.system_view_mode));

    // Find which systems fit from top_pos downward.
    let visible = visible_range(order, state, top_pos, usable_height);

    if visible.contains(&selected_pos) {
        // Already visible, nothing to do.
        return;
    }

    // If selected is above viewport, scroll up.
    if selected_pos < top_pos {
        state.viewport_top_id = Some(state.systems[order[selected_pos]].id.clone());
        return;
    }

    // If selected is below viewport, move the top only as far as necessary.
    if selected_pos >= top_pos {
        let mut candidate = selected_pos;
        while candidate > top_pos {
            let previous = candidate - 1;
            let range = visible_range(order, state, previous, usable_height);
            if range.contains(&selected_pos) {
                candidate = previous;
            } else {
                break;
            }
        }
        state.viewport_top_id = Some(state.systems[order[candidate]].id.clone());
    }
}

/// Rows reserved above the entries by a view.
#[must_use]
pub const fn view_header_height(system_view_mode: SystemViewMode) -> u16 {
    match system_view_mode {
        SystemViewMode::Normal => 0,
        SystemViewMode::Condensed => 2,
    }
}

fn valid_drive_count(system: &SystemState) -> u16 {
    system
        .latest
        .as_ref()
        .and_then(|snapshot| snapshot.drives.as_deref())
        .map_or(0, valid_drive_count_from_slice)
}

fn valid_drive_count_from_slice(drives: &[crate::normalized::NormalizedDrive]) -> u16 {
    drives
        .iter()
        .filter(|drive| drive.total_bytes > 0 && drive.used_bytes <= drive.total_bytes)
        .count()
        .try_into()
        .unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EggpoolEntry, EggpoolScheme, SystemEntry};
    use gregg_protocol::test_support::LinuxSnapshotBuilder;
    use gregg_protocol::StatusSnapshot;

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

    fn eggpool_config(with_system: bool) -> Config {
        let mut config = if with_system {
            test_config_with_ids(&["system"])
        } else {
            Config::default()
        };
        config.eggpool = Some(EggpoolEntry {
            id: "eggpool-id".into(),
            host: "pool.local".into(),
            port: 11300,
            scheme: EggpoolScheme::Http,
            name: Some("Main EggPool".into()),
            api_key_env: None,
        });
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
    fn from_config_preserves_configured_endpoint_host_exactly() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "exact".into(),
            host: "192.168.183.143".into(),
            port: 11310,
            name: None,
        });

        let state = AppState::from_config(&config);
        assert_eq!(state.systems[0].endpoint.host, "192.168.183.143");
    }

    #[test]
    fn reconcile_systems_replaces_targets_preserves_unchanged_state_and_repairs_ids() {
        let old_config = Config {
            systems: vec![
                SystemEntry {
                    id: "changed".into(),
                    host: "192.168.182.143".into(),
                    port: 11310,
                    name: Some("Old".into()),
                },
                SystemEntry {
                    id: "same".into(),
                    host: "same.local".into(),
                    port: 11311,
                    name: Some("Same".into()),
                },
                SystemEntry {
                    id: "removed".into(),
                    host: "removed.local".into(),
                    port: 11312,
                    name: None,
                },
            ],
            ..Config::default()
        };
        let mut state = AppState::from_config(&old_config);
        state.selected_id = Some("removed".into());

        let first_batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: state
                .systems
                .iter()
                .take(2)
                .map(|system| crate::poller::PollResult {
                    system_id: system.id.clone(),
                    endpoint: system.endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(25),
                })
                .collect(),
        };
        state.apply_batch(&first_batch);
        state.systems[0].last_error = Some(PollOutcome::Timeout);

        let retained_snapshot = state.systems[1].latest.clone();
        let retained_success = state.systems[1].last_success_at;
        let new_config = Config {
            systems: vec![
                SystemEntry {
                    id: "changed".into(),
                    host: "192.168.183.143".into(),
                    port: 11310,
                    name: Some("New".into()),
                },
                SystemEntry {
                    id: "same".into(),
                    host: "same.local".into(),
                    port: 11311,
                    name: Some("Renamed".into()),
                },
                SystemEntry {
                    id: "added".into(),
                    host: "added.local".into(),
                    port: 11313,
                    name: None,
                },
            ],
            ..old_config.clone()
        };

        state.reconcile_systems(&new_config);

        assert_eq!(state.systems.len(), 3);
        assert_eq!(state.systems[0].endpoint.host, "192.168.183.143");
        assert_eq!(state.systems[0].configured_name.as_deref(), Some("New"));
        assert_eq!(state.systems[0].reachability, Reachability::Pending);
        assert!(state.systems[0].latest.is_none());
        assert!(state.systems[0].last_success_at.is_none());
        assert!(state.systems[0].last_attempt_at.is_none());
        assert!(state.systems[0].latency.is_none());
        assert!(state.systems[0].last_error.is_none());

        assert_eq!(state.systems[1].configured_name.as_deref(), Some("Renamed"));
        assert_eq!(state.systems[1].reachability, Reachability::Online);
        assert_eq!(state.systems[1].latest, retained_snapshot);
        assert_eq!(state.systems[1].last_success_at, retained_success);
        assert_eq!(state.selected_id.as_deref(), Some("changed"));
        assert_eq!(state.viewport_top_id.as_deref(), Some("changed"));
        assert_eq!(state.systems[2].id, "added");
        assert_eq!(state.systems[2].reachability, Reachability::Pending);
    }

    #[test]
    fn reconcile_systems_preserves_state_when_dns_host_case_changes() {
        let config = Config {
            systems: vec![SystemEntry {
                id: "same".into(),
                host: "Server.Local".into(),
                port: 11310,
                name: None,
            }],
            ..Config::default()
        };
        let mut state = AppState::from_config(&config);
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "same".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(10),
            }],
        });

        state.reconcile_systems(&Config {
            systems: vec![SystemEntry {
                id: "same".into(),
                host: "server.local".into(),
                port: 11310,
                name: None,
            }],
            ..Config::default()
        });

        assert_eq!(state.systems[0].reachability, Reachability::Online);
        assert!(state.systems[0].latest.is_some());
    }

    #[test]
    fn apply_batch_rejects_result_from_superseded_endpoint() {
        let mut config = test_config_with_ids(&["a"]);
        config.systems[0].host = "new.local".into();
        let mut state = AppState::from_config(&config);
        let old_endpoint = Endpoint::new("old.local".into(), 11310, None);
        state.systems[0].endpoint = old_endpoint.clone();
        state.reconcile_systems(&config);

        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: old_endpoint,
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(1),
            }],
        });

        assert_eq!(state.systems[0].endpoint.host, "new.local");
        assert_eq!(state.systems[0].reachability, Reachability::Pending);
        assert!(state.systems[0].latest.is_none());
    }

    #[test]
    fn apply_batch_accepts_the_single_generation_wrap_after_max() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::from_config(&config);
        state.last_applied_generation = u64::MAX;
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: Vec::new(),
        });
        assert_eq!(state.last_applied_generation, 1);
    }

    #[test]
    fn apply_batch_rejects_a_skipped_generation_wrap() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::from_config(&config);
        state.last_applied_generation = u64::MAX - 1;
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: Vec::new(),
        });
        assert_eq!(state.last_applied_generation, u64::MAX - 1);
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
    fn pane_initialization_and_cycling_follow_configured_sources() {
        let systems = AppState::from_config(&test_config_with_ids(&["a"]));
        assert_eq!(systems.active_pane, Pane::Systems);
        let eggpool = AppState::from_config(&eggpool_config(false));
        assert_eq!(eggpool.active_pane, Pane::Eggpool);
        assert!(eggpool.eggpool.is_some());

        let mut both = AppState::from_config(&eggpool_config(true));
        both.apply_action(Action::NextPane);
        assert_eq!(both.active_pane, Pane::Eggpool);
        both.apply_action(Action::PreviousPane);
        assert_eq!(both.active_pane, Pane::Systems);
    }

    #[test]
    fn eggpool_period_movement_is_bounded_and_invalidates_old_summary() {
        let mut state = AppState::from_config(&eggpool_config(false));
        assert_eq!(state.eggpool.as_ref().unwrap().period, EggpoolPeriod::Hour);
        state.apply_action(Action::MoveUp);
        assert_eq!(state.eggpool.as_ref().unwrap().period, EggpoolPeriod::Hour);
        state.apply_action(Action::MoveDown);
        state.apply_action(Action::MoveDown);
        state.apply_action(Action::MoveDown);
        state.apply_action(Action::MoveDown);
        let eggpool = state.eggpool.as_ref().unwrap();
        assert_eq!(eggpool.period, EggpoolPeriod::Month);
        assert_eq!(eggpool.request_generation, 3);
        assert!(eggpool.summary.is_none());
    }

    #[test]
    fn eggpool_results_reject_stale_or_mismatched_requests_and_retain_same_period_failures() {
        let mut state = AppState::from_config(&eggpool_config(false));
        state.apply_action(Action::MoveDown);
        let now = Instant::now();
        let summary = EggpoolSummary {
            accounted_tokens: 42,
            cache_read_ratio: Some(0.5),
            output_tokens_per_second: 2.0,
            avg_ttft_ms: Some(12.0),
            period: EggpoolPeriod::Day,
        };
        let result = |generation, period, outcome| EggpoolResult {
            generation,
            period,
            started_at: now,
            completed_at: now,
            outcome,
        };
        state.apply_eggpool_result(&result(
            0,
            EggpoolPeriod::Day,
            EggpoolFetchOutcome::Online(summary.clone()),
        ));
        assert!(state.eggpool.as_ref().unwrap().summary.is_none());
        state.apply_eggpool_result(&result(
            1,
            EggpoolPeriod::Hour,
            EggpoolFetchOutcome::Online(summary.clone()),
        ));
        assert!(state.eggpool.as_ref().unwrap().summary.is_none());
        state.apply_eggpool_result(&result(
            1,
            EggpoolPeriod::Day,
            EggpoolFetchOutcome::Online(summary),
        ));
        assert!(state.eggpool.as_ref().unwrap().summary.is_some());
        state.apply_eggpool_result(&result(1, EggpoolPeriod::Day, EggpoolFetchOutcome::Timeout));
        assert!(state.eggpool.as_ref().unwrap().summary.is_some());
        assert!(matches!(
            state.eggpool.as_ref().unwrap().last_error,
            Some(EggpoolFetchOutcome::Timeout)
        ));
        state.apply_eggpool_result(&result(
            1,
            EggpoolPeriod::Day,
            EggpoolFetchOutcome::Online(EggpoolSummary {
                accounted_tokens: 43,
                cache_read_ratio: None,
                output_tokens_per_second: 3.0,
                avg_ttft_ms: None,
                period: EggpoolPeriod::Day,
            }),
        ));
        assert_eq!(
            state
                .eggpool
                .as_ref()
                .unwrap()
                .summary
                .as_ref()
                .unwrap()
                .accounted_tokens,
            43
        );
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

        state.apply_action(Action::MoveDown);
        assert_eq!(state.selected_id.as_deref(), Some("b"));

        state.apply_action(Action::MoveDown);
        assert_eq!(state.selected_id.as_deref(), Some("c"));

        // Should clamp at the end.
        state.apply_action(Action::MoveDown);
        assert_eq!(state.selected_id.as_deref(), Some("c"));
    }

    #[test]
    fn select_previous_moves_backward() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);

        state.apply_action(Action::MoveDown);
        state.apply_action(Action::MoveDown);
        assert_eq!(state.selected_id.as_deref(), Some("c"));

        state.apply_action(Action::MoveUp);
        assert_eq!(state.selected_id.as_deref(), Some("b"));

        state.apply_action(Action::MoveUp);
        assert_eq!(state.selected_id.as_deref(), Some("a"));

        // Should clamp at the beginning.
        state.apply_action(Action::MoveUp);
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
    fn page_movement_is_noop_when_no_entry_fits() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);
        state.terminal_size = Some((80, 0));

        state.apply_action(Action::PageDown);
        assert_eq!(state.selected_id.as_deref(), Some("a"));

        state.apply_action(Action::PageUp);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
    }

    #[test]
    fn move_selection_tolerates_isize_min_offset() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);
        let order = state.display_order();

        // Negating `isize::MIN` would overflow; the helper must clamp
        // rather than panic.
        state.move_selection(&order, isize::MIN);
        assert_eq!(state.selected_id.as_deref(), Some("a"));

        state.move_selection(&order, isize::MAX);
        assert_eq!(state.selected_id.as_deref(), Some("c"));
    }

    #[test]
    fn selection_preserved_across_reorder() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);

        // Move past the launch-initialization phase so later batches
        // observe ordinary selection/scroll semantics, matching the
        // production flow (per Phase 083 the *first* accepted batch
        // pins selection and viewport to display-order position zero).
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![],
        });
        assert_eq!(state.last_applied_generation, 1);

        // Select b.
        state.apply_action(Action::MoveDown);
        assert_eq!(state.selected_id.as_deref(), Some("b"));

        // Make a online (changes display order but b is still selected).
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

        assert_eq!(state.selected_id.as_deref(), Some("b"));
    }

    #[test]
    fn first_batch_snaps_selection_and_viewport_to_display_order_top() {
        // Phase 083: a fresh launch must place selection and viewport
        // at display-order position zero after the first accepted
        // batch — an offline first-configured system must not pull the
        // viewport below later online systems.
        let config = test_config_with_ids(&["offline0", "online2", "offline1"]);
        let mut state = AppState::from_config(&config);
        // Operator already scrolled before the first poll arrives.
        state.apply_action(Action::SelectLast);
        assert_eq!(state.selected_id.as_deref(), Some("offline1"));

        let endpoints: Vec<_> = state
            .systems
            .iter()
            .map(|system| system.endpoint.clone())
            .collect();
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: endpoints
                .iter()
                .enumerate()
                .map(|(idx, endpoint)| {
                    let outcome = if idx == 1 {
                        PollOutcome::Online(Box::new(make_snapshot()))
                    } else {
                        PollOutcome::ConnectionRefused
                    };
                    crate::poller::PollResult {
                        system_id: state.systems[idx].id.clone(),
                        endpoint: endpoint.clone(),
                        outcome,
                        latency: Duration::from_millis(50),
                    }
                })
                .collect(),
        };
        state.apply_batch(&batch);

        let order = state.display_order();
        let first_id = &state.systems[order[0]].id;
        assert_eq!(state.selected_id.as_deref(), Some(first_id.as_str()));
        assert_eq!(state.viewport_top_id.as_deref(), Some(first_id.as_str()));
        // online2 is the only online system, must be at the top.
        assert_eq!(first_id, "online2");
    }

    #[test]
    fn subsequent_batches_do_not_reset_selection_to_top() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);
        // First batch initializes the session.
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![],
        });
        // Pick something offline on purpose to verify reachability does
        // not drive the second-batch reset.
        state.apply_action(Action::SelectLast);
        let chosen = state.selected_id.clone();
        assert_eq!(chosen.as_deref(), Some("c"));

        let endpoint = state.systems[0].endpoint.clone();
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint,
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(50),
            }],
        });

        // The existing offline selection survives the second batch.
        assert_eq!(state.selected_id, chosen);
    }

    #[test]
    fn entry_height_online_is_five() {
        let mut state = AppState {
            systems: vec![SystemState {
                id: "test".into(),
                endpoint: Endpoint::new("host".into(), 11310, None),
                configured_name: None,
                reachability: Reachability::Online,
                latest: None,
                last_success_at: None,
                last_attempt_at: None,
                latency: None,
                last_error: None,
            }],
            selected_id: Some("test".into()),
            viewport_top_id: Some("test".into()),
            last_applied_generation: 0,
            refresh_status: RefreshStatus::Idle,
            config_reload_error: None,
            terminal_size: None,
            active_pane: Pane::Systems,
            system_view_mode: SystemViewMode::Normal,
            drives_expanded: false,
            selection_highlight_active: false,
            eggpool: None,
        };
        assert_eq!(entry_height(&state, 0), 5);

        state.systems[0].reachability = Reachability::Pending;
        assert_eq!(entry_height(&state, 0), 1);

        state.systems[0].reachability = Reachability::Offline;
        assert_eq!(entry_height(&state, 0), 1);
    }

    #[test]
    fn visible_range_handles_mixed_heights() {
        let config = test_config_with_ids(&["a", "b", "c", "d", "e"]);
        let state = AppState::from_config(&config);
        let order = state.display_order();
        let range = visible_range(&order, &state, 0, 20);
        // Should include some entries.
        assert!(!range.is_empty());
    }

    #[test]
    fn visible_range_small_terminal() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);
        state.systems[0].reachability = Reachability::Online;
        let order = state.display_order();
        let range = visible_range(&order, &state, 0, 3);
        // Terminal too small for even one online entry.
        assert!(range.is_empty());
    }

    #[test]
    fn visible_range_online_boundary_is_five_rows() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::from_config(&config);
        state.systems[0].reachability = Reachability::Online;
        let order = state.display_order();

        assert!(visible_range(&order, &state, 0, 4).is_empty());
        assert_eq!(visible_range(&order, &state, 0, 5), 0..1);
    }

    #[test]
    fn visible_range_first_offline_entry_does_not_reserve_online_height() {
        let config = test_config_with_ids(&["offline", "online"]);
        let mut state = AppState::from_config(&config);
        state.systems[1].reachability = Reachability::Online;
        let order = vec![0, 1];

        assert_eq!(visible_range(&order, &state, 0, 1), 0..1);
    }

    #[test]
    fn visible_range_expanded_online_entry_clips_only_drive_rows() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::from_config(&config);
        state.systems[0].reachability = Reachability::Online;
        state.systems[0].latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        state.systems[0].latest.as_mut().unwrap().drives = Some(
            (0..3)
                .map(|index| crate::normalized::NormalizedDrive {
                    name: format!("drive{index}"),
                    used_bytes: 1,
                    total_bytes: 2,
                    available_bytes: None,
                })
                .collect(),
        );
        state.selected_id = Some("a".into());
        state.drives_expanded = true;
        let order = state.display_order();

        assert_eq!(visible_range(&order, &state, 0, 5), 0..1);
        assert_eq!(visible_range(&order, &state, 0, 6), 0..1);
        assert_eq!(entry_height(&state, 0), 8);

        let viewport = crate::ui::layout::compute_viewport(
            &state,
            ratatui::layout::Rect::new(0, 0, 80, 5),
            &order,
        );
        assert_eq!(viewport[0].drive_rows_visible, 0);
        let viewport = crate::ui::layout::compute_viewport(
            &state,
            ratatui::layout::Rect::new(0, 0, 80, 6),
            &order,
        );
        assert_eq!(viewport[0].drive_rows_visible, 1);
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
    fn selection_stays_visible_across_dynamic_online_entries() {
        let config = test_config_with_ids(&["a", "b", "c", "d"]);
        let mut state = AppState::from_config(&config);
        state.terminal_size = Some((80, 10));
        for system in &mut state.systems {
            system.reachability = Reachability::Online;
            system.latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        }

        state.apply_action(Action::SelectLast);
        let order = state.display_order();
        let top = order
            .iter()
            .position(|&index| state.systems[index].id == state.viewport_top_id.clone().unwrap())
            .unwrap();
        let selected = order
            .iter()
            .position(|&index| state.systems[index].id == state.selected_id.clone().unwrap())
            .unwrap();
        assert_eq!(top, 2);
        assert!(visible_range(&order, &state, top, 10).contains(&selected));

        state.apply_action(Action::MoveUp);
        assert_eq!(state.viewport_top_id.as_deref(), Some("c"));
    }

    #[test]
    fn expansion_changes_only_selected_entry_height() {
        let config = test_config_with_ids(&["a", "b"]);
        let mut state = AppState::from_config(&config);
        for system in &mut state.systems {
            system.reachability = Reachability::Online;
            system.latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        }
        state.systems[0].latest.as_mut().unwrap().drives =
            Some(vec![crate::normalized::NormalizedDrive {
                name: "/".into(),
                used_bytes: 1,
                total_bytes: 2,
                available_bytes: None,
            }]);
        assert_eq!(entry_height(&state, 0), 5);
        assert_eq!(entry_height(&state, 1), 5);
        state.apply_action(Action::ToggleDrives);
        assert_eq!(entry_height(&state, 0), 6);
        assert_eq!(entry_height(&state, 1), 5);
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

        state.apply_action(Action::MoveDown);
        assert!(state.selected_id.is_none());

        state.apply_action(Action::MoveUp);
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

    #[test]
    fn view_controls_wrap_and_preserve_selection_and_expansion() {
        let config = test_config_with_ids(&["a", "b"]);
        let mut state = AppState::from_config(&config);
        state.terminal_size = Some((80, 8));
        state.systems[0].reachability = Reachability::Online;
        state.systems[0].latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        state.selected_id = Some("a".into());

        state.apply_action(Action::ToggleDrives);
        state.apply_action(Action::ToggleSystemView);
        assert_eq!(state.system_view_mode, SystemViewMode::Condensed);
        assert!(state.drives_expanded);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
        state.apply_action(Action::ToggleSystemView);
        assert_eq!(state.system_view_mode, SystemViewMode::Normal);
        assert!(state.drives_expanded);
    }

    #[test]
    fn condensed_expansion_counts_only_valid_drive_rows() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::from_config(&config);
        state.system_view_mode = SystemViewMode::Condensed;
        state.drives_expanded = true;
        state.systems[0].reachability = Reachability::Online;
        let mut snapshot = NormalizedSnapshot::from_v1(&make_snapshot());
        snapshot.drives = Some(vec![
            crate::normalized::NormalizedDrive {
                name: "/".into(),
                used_bytes: 1,
                total_bytes: 2,
                available_bytes: None,
            },
            crate::normalized::NormalizedDrive {
                name: "/bad".into(),
                used_bytes: 3,
                total_bytes: 2,
                available_bytes: None,
            },
        ]);
        state.systems[0].latest = Some(snapshot);
        assert_eq!(entry_height(&state, 0), 2);
    }
}
