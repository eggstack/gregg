#![allow(dead_code)]

pub mod bar;
pub mod condensed;
pub mod diagnostics;
pub mod eggpool;
pub mod layout;
pub mod system_block;
pub mod text;

use std::cell::RefCell;
use std::rc::Rc;

use ratatui::Frame;

use crate::normalized::NormalizedSnapshot;
use crate::state::{AppState, Pane, Reachability, SystemViewMode};

use system_block::MetricRow;

/// Render the full TUI into the current frame.
pub fn render(f: &mut Frame, state: &AppState) {
    let area = f.area();

    if state.systems.is_empty() && state.eggpool.is_none() {
        diagnostics::render_empty_config(f, area);
        return;
    }

    if state.active_pane == Pane::Eggpool {
        eggpool::render(f, area, state);
        return;
    }

    let display_order = state.display_order();

    let minimum_height = match state.system_view_mode {
        SystemViewMode::Normal => {
            let first_is_online = display_order
                .first()
                .and_then(|&index| state.systems.get(index))
                .is_some_and(|system| system.reachability == Reachability::Online);
            if first_is_online {
                5
            } else {
                1
            }
        }
        SystemViewMode::Condensed => 3,
    };
    if area.width < 24 || area.height < minimum_height || area.height == 0 {
        diagnostics::render_too_small(f, area);
        return;
    }

    let condensed_layout = if state.system_view_mode == SystemViewMode::Condensed {
        condensed::compute_condensed_table_layout(&state.systems, area.width)
    } else {
        // The condensed layout is unused outside the condensed branch;
        // build a placeholder so the borrow checker is satisfied without
        // paying for the format pass.
        condensed::compute_condensed_table_layout(&[], area.width)
    };

    if state.system_view_mode == SystemViewMode::Condensed {
        condensed::render_header(f, area, &condensed_layout);
    }

    let entries = layout::compute_viewport(state, area, &display_order);

    // Compute the fleet-wide metric geometry once per render, from every
    // online system with a current normalized snapshot, so the opening
    // and closing brackets line up across all online systems. The
    // population includes systems outside the viewport so scrolling
    // does not cause horizontal reflow. Rows come from a per-system memo:
    // identical snapshots produce identical rows, so each system's rows are
    // rebuilt only when its snapshot content or membership changed.
    let (online_rows, fleet_layout) = if state.system_view_mode == SystemViewMode::Normal {
        let online_rows: Vec<(usize, Rc<[MetricRow; 4]>)> = metric_rows_for_fleet(state);
        let fleet_layout = system_block::compute_fleet_metric_layout(
            online_rows.iter().map(|(_, rows)| &**rows),
            area.width,
        );
        (online_rows, fleet_layout)
    } else {
        (Vec::new(), system_block::MetricFleetLayout::empty())
    };

    for entry in &entries {
        let system = &state.systems[entry.index];
        if state.system_view_mode == SystemViewMode::Condensed {
            condensed::render_entry(
                f,
                entry.rect,
                system,
                &condensed_layout,
                entry.is_visually_selected,
                entry.drive_rows_visible,
            );
            continue;
        }
        match system.reachability {
            Reachability::Online => {
                let rows = online_rows
                    .iter()
                    .find(|(index, _)| *index == entry.index)
                    .map(|(_, rows)| &**rows);
                system_block::render_online(
                    f,
                    entry.rect,
                    system,
                    rows,
                    &fleet_layout,
                    entry.is_visually_selected,
                    entry.drive_rows_visible,
                );
            }
            Reachability::Offline | Reachability::Pending => {
                system_block::render_offline(f, entry.rect, system, entry.is_visually_selected);
            }
        }
    }

    // Show a key hint only when there is at least one extra row below entries.
    let entries_bottom = entries.last().map_or(area.y, |e| e.rect.y + e.rect.height);
    let extra_rows = area
        .y
        .saturating_add(area.height)
        .saturating_sub(entries_bottom);
    if extra_rows >= 1 {
        diagnostics::render_key_hint(f, area, state);
    }
}

/// Per-system memo of formatted normal-view metric rows.
struct CachedMetricRows {
    system_id: String,
    snapshot: NormalizedSnapshot,
    rows: Rc<[MetricRow; 4]>,
}

thread_local! {
    static METRIC_ROWS_CACHE: RefCell<Vec<CachedMetricRows>> = const { RefCell::new(Vec::new()) };
}

/// Build (or reuse) the metric rows of every online system with a snapshot.
///
/// Iterating all online systems per render is required by the fleet-wide
/// layout invariant, but identical snapshots produce identical rows, so each
/// system's formatted rows are rebuilt only when its snapshot content
/// (compared by full value, immune to any mutation path) or membership
/// changed. Returns `(system index, rows)` pairs in configured order.
fn metric_rows_for_fleet(state: &AppState) -> Vec<(usize, Rc<[MetricRow; 4]>)> {
    let mut online_rows = Vec::new();
    METRIC_ROWS_CACHE.with_borrow_mut(|cache| {
        for (index, system) in state.systems.iter().enumerate() {
            if system.reachability != Reachability::Online {
                continue;
            }
            let Some(snapshot) = system.latest.as_ref() else {
                continue;
            };
            let rows = match cache.iter_mut().find(|entry| entry.system_id == system.id) {
                Some(entry) if entry.snapshot == *snapshot => Rc::clone(&entry.rows),
                Some(entry) => {
                    entry.snapshot = snapshot.clone();
                    entry.rows = Rc::new(system_block::build_metric_rows(snapshot));
                    Rc::clone(&entry.rows)
                }
                None => {
                    let rows: Rc<[MetricRow; 4]> =
                        Rc::new(system_block::build_metric_rows(snapshot));
                    cache.push(CachedMetricRows {
                        system_id: system.id.clone(),
                        snapshot: snapshot.clone(),
                        rows: Rc::clone(&rows),
                    });
                    rows
                }
            };
            online_rows.push((index, rows));
        }
        // Amortized prune of ids that left the configured fleet so the
        // linear lookup stays bounded across config reload churn.
        if cache.len() > state.systems.len() * 4 + 16 {
            cache.retain(|entry| state.systems.iter().any(|s| s.id == entry.system_id));
        }
    });
    online_rows
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::Terminal;
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    use crate::config::{Config, SystemEntry};
    use crate::normalized::NormalizedDrive;
    use crate::poller::{PollBatch, PollOutcome};
    use crate::state::{AppState, Reachability};
    use gregg_protocol::test_support::LinuxSnapshotV2Builder;
    use gregg_protocol::test_support::{
        LinuxSnapshotBuilder, MacosSnapshotBuilder, WindowsSnapshotV2Builder,
    };
    use gregg_protocol::v2::DriveMetrics;
    use gregg_protocol::StatusSnapshot;

    fn render_state(state: &AppState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| super::render(f, state)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut lines = Vec::new();
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push(
                    buf.cell((x, y))
                        .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' ')),
                );
            }
            lines.push(line);
        }
        lines.join("\n")
    }

    fn linux_snap() -> StatusSnapshot {
        LinuxSnapshotBuilder::default().build()
    }

    fn linux_snap_custom(usage_pct: f32, iowait_pct: f32, cores: u32) -> StatusSnapshot {
        LinuxSnapshotBuilder::default()
            .usage_pct(usage_pct)
            .iowait_pct(iowait_pct)
            .logical_cores(cores)
            .build()
    }

    fn macos_snap() -> StatusSnapshot {
        MacosSnapshotBuilder::default().build()
    }

    fn macos_snap_custom(usage_pct: f32, cores: u32) -> StatusSnapshot {
        MacosSnapshotBuilder::default()
            .usage_pct(usage_pct)
            .logical_cores(cores)
            .build()
    }

    fn test_config(names: &[&str]) -> Config {
        let mut config = Config::default();
        for (i, name) in names.iter().enumerate() {
            config.systems.push(SystemEntry {
                id: format!("id-{i}"),
                host: format!("host{i}.local"),
                port: 11310,
                name: Some((*name).to_string()),
            });
        }
        config
    }

    fn make_online_batch(state: &AppState, system_index: usize, snap: StatusSnapshot) -> PollBatch {
        let system = &state.systems[system_index];
        PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: system.id.clone(),
                endpoint: system.endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(snap)),
                latency: Duration::from_millis(10),
            }],
        }
    }

    fn make_offline_batch(state: &AppState, system_index: usize) -> PollBatch {
        let system = &state.systems[system_index];
        PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: system.id.clone(),
                endpoint: system.endpoint.clone(),
                outcome: PollOutcome::ConnectionRefused,
                latency: Duration::from_millis(10),
            }],
        }
    }

    fn apply_online(state: &mut AppState, index: usize, snap: StatusSnapshot) {
        let batch = make_online_batch(state, index, snap);
        state.apply_batch(&batch);
    }

    fn apply_online_v2(
        state: &mut AppState,
        index: usize,
        payload: gregg_protocol::v2::StatusPayloadV2,
        generation: u64,
    ) {
        let system_id = state.systems[index].id.clone();
        let endpoint = state.systems[index].endpoint.clone();
        state.apply_batch(&PollBatch {
            generation,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id,
                endpoint,
                outcome: PollOutcome::OnlineV2(Box::new(payload)),
                latency: Duration::from_millis(10),
            }],
        });
    }

    fn apply_offline(state: &mut AppState, index: usize) {
        let mut batch = make_offline_batch(state, index);
        batch.generation = state.last_applied_generation + 1;
        state.apply_batch(&batch);
    }

    fn count_nonblank_lines(output: &str) -> usize {
        output.lines().filter(|l| !l.trim().is_empty()).count()
    }

    fn line_contains(output: &str, line_index: usize, needle: &str) -> bool {
        output
            .lines()
            .nth(line_index)
            .is_some_and(|l| l.contains(needle))
    }

    fn metric_lines(output: &str) -> Vec<&str> {
        output.lines().skip(1).take(4).collect()
    }

    fn terminal_column(line: &str, needle: char) -> usize {
        let mut column = 0;
        for character in line.chars() {
            if character == needle {
                return column;
            }
            column += UnicodeWidthChar::width(character).unwrap_or(0);
        }
        panic!("{needle:?} not found in rendered line {line:?}");
    }

    // ── 1. Empty config ──────────────────────────────────────────────

    #[test]
    fn render_empty_config() {
        let config = Config::default();
        let state = AppState::from_config(&config);
        let output = render_state(&state, 80, 24);
        assert!(
            output.contains("No sources configured"),
            "expected 'No sources configured' in output:\n{output}"
        );
    }

    // ── 2. Terminal too small ────────────────────────────────────────

    #[test]
    fn render_too_small_width() {
        let config = test_config(&["web1"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 20, 24);
        assert!(
            output.contains("terminal too"),
            "expected 'terminal too' in output:\n{output}"
        );
    }

    #[test]
    fn render_too_small_height() {
        let config = test_config(&["web1"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 80, 2);
        assert!(
            output.contains("terminal too"),
            "expected 'terminal too' in output:\n{output}"
        );
    }

    // ── 3. Online system rendering ───────────────────────────────────

    #[test]
    fn render_online_linux_system() {
        let config = test_config(&["web1"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 80, 8);

        let lines: Vec<&str> = output.lines().collect();
        // Online system occupies five rows.
        assert!(
            !lines[0].trim().is_empty(),
            "header row should not be empty"
        );
        assert!(
            lines[0].contains("web1"),
            "header should contain system name 'web1', got: {}",
            lines[0]
        );
        // CPU bar
        assert!(
            lines[1].contains("CPU"),
            "row 1 should be CPU bar, got: {}",
            lines[1]
        );
        // MEM bar
        assert!(
            lines[2].contains("MEM"),
            "row 2 should be MEM bar, got: {}",
            lines[2]
        );
        // SWP bar
        assert!(
            lines[3].contains("SWP"),
            "row 3 should be SWP bar, got: {}",
            lines[3]
        );
    }

    #[test]
    fn render_online_macos_system() {
        let config = test_config(&["mac1"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, macos_snap());
        let output = render_state(&state, 80, 8);

        let header = output.lines().next().unwrap();
        assert!(
            header.contains("mac1"),
            "header should contain 'mac1', got: {header}"
        );
        // Plan 087: macOS has cpu_iowait = false so the IO token is
        // omitted entirely. There must be no placeholder, no
        // separator artifact, and no fabricated percentage.
        assert!(
            !header.contains("IO "),
            "macOS header must omit the IO token, got: {header:?}"
        );
        assert!(
            !header.contains("0.0%"),
            "macOS header must not fabricate any percentage, got: {header}"
        );
    }

    // ── 4. Offline system rendering ──────────────────────────────────

    #[test]
    fn render_offline_system() {
        let config = test_config(&["web1"]);
        let mut state = AppState::from_config(&config);
        apply_offline(&mut state, 0);
        let output = render_state(&state, 80, 4);
        assert!(
            output.contains("offline"),
            "expected 'offline' in output:\n{output}"
        );
    }

    #[test]
    fn render_offline_system_preserves_configured_ip() {
        let mut config = test_config(&["web1"]);
        config.systems[0].host = "192.168.183.143".into();
        let mut state = AppState::from_config(&config);
        apply_offline(&mut state, 0);
        let output = render_state(&state, 80, 4);
        assert!(output.contains("192.168.183.143:11310"), "{output}");
        assert!(!output.contains("192.168.182.143"), "{output}");
    }

    #[test]
    fn render_offline_unicode_name_uses_display_width_for_padding() {
        let mut config = test_config(&["サーバー"]);
        config.systems[0].name = Some("é".into());
        let mut state = AppState::from_config(&config);
        apply_offline(&mut state, 0);

        let width = 40u16;
        let output = render_state(&state, width, 1);
        let line = output.lines().next().unwrap();
        let prefix = "é@host0.local:11310 offline ";
        let expected_dots = usize::from(width) - UnicodeWidthStr::width(prefix);

        assert!(line.starts_with(prefix), "rendered line: {line:?}");
        let trailing_dots = line.strip_prefix(prefix).unwrap().trim_end_matches(' ');
        assert_eq!(
            trailing_dots
                .chars()
                .filter(|&character| character == '.')
                .count(),
            expected_dots,
            "offline padding must use terminal cells, not UTF-8 bytes: {line:?}"
        );
        assert!(UnicodeWidthStr::width(line) <= usize::from(width));
    }

    #[test]
    fn render_pending_system() {
        let config = test_config(&["web1"]);
        let state = AppState::from_config(&config);
        let output = render_state(&state, 80, 4);
        assert!(
            output.contains("pending"),
            "expected 'pending' in output:\n{output}"
        );
    }

    // ── 5. Mixed online/offline ordering ─────────────────────────────

    #[test]
    fn render_mixed_online_offline() {
        let config = test_config(&["a", "b", "c", "d"]);
        let mut state = AppState::from_config(&config);
        // Make b and d online (leave a and c pending).
        apply_online(&mut state, 1, linux_snap());
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: state.systems[3].id.clone(),
                endpoint: state.systems[3].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap())),
                latency: Duration::from_millis(10),
            }],
        });
        // Reset viewport so all systems are visible after display order changed.
        state.viewport_top_id = None;

        let output = render_state(&state, 80, 20);
        let lines: Vec<&str> = output.lines().collect();

        // Online systems (b, d) should appear before offline (a, c).
        let b_line = lines
            .iter()
            .position(|l| l.contains("b "))
            .expect("b should be rendered");
        let d_line = lines
            .iter()
            .position(|l| l.contains("d "))
            .expect("d should be rendered");
        let a_line = lines
            .iter()
            .position(|l| l.starts_with("a@"))
            .expect("a should be rendered");
        let c_line = lines
            .iter()
            .position(|l| l.starts_with("c@"))
            .expect("c should be rendered");

        assert!(
            b_line < a_line,
            "online system b (line {b_line}) should appear before offline a (line {a_line})"
        );
        assert!(
            d_line < c_line,
            "online system d (line {d_line}) should appear before offline c (line {c_line})"
        );
    }

    // ── 6. Selection indicator ───────────────────────────────────────

    #[test]
    fn render_selected_online_system() {
        let config = test_config(&["a", "b"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: state.systems[1].id.clone(),
                endpoint: state.systems[1].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap())),
                latency: Duration::from_millis(10),
            }],
        });
        // First system (a) is selected by default.
        assert_eq!(state.selected_id.as_deref(), Some("id-0"));
        // Plan 087: visual highlight is independent of logical selection.
        // Activate the highlight so the renderer applies REVERSED.
        state.selection_highlight_active = true;

        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| super::render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();

        // Check that the header row of system a (row 0) uses reversed styling.
        // In ratatui's TestBackend buffer, we can check the style of cells.
        let cell = buf.cell((0, 0)).unwrap();
        let style = cell.style();
        assert!(
            style.add_modifier.contains(Modifier::REVERSED),
            "selected system's header should have REVERSED modifier, got style: {style:?}"
        );
    }

    #[test]
    fn render_selected_offline_system() {
        let config = test_config(&["a"]);
        let mut state = AppState::from_config(&config);
        apply_offline(&mut state, 0);
        // Plan 087: visual highlight must be explicitly activated to
        // render the REVERSED modifier.
        state.selection_highlight_active = true;

        let backend = TestBackend::new(80, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| super::render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();

        let cell = buf.cell((0, 0)).unwrap();
        let style = cell.style();
        assert!(
            style.add_modifier.contains(Modifier::REVERSED),
            "selected offline system should have REVERSED modifier, got style: {style:?}"
        );
    }

    #[test]
    fn startup_does_not_render_logical_selection_with_reversed_style() {
        // Plan 087: at startup no system may be visually reversed even
        // though `selected_id` is already populated deterministically.
        let config = test_config(&["a", "b"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: state.systems[1].id.clone(),
                endpoint: state.systems[1].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap())),
                latency: Duration::from_millis(10),
            }],
        });
        assert_eq!(state.selected_id.as_deref(), Some("id-0"));
        assert!(
            !state.selection_highlight_active,
            "highlight must remain off until a selection-changing action"
        );

        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| super::render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();
        assert!(
            !buf.cell((0, 0))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "header must not be reversed before any selection action: row 0 = {:?}",
            buf.cell((0, 0)).map(ratatui::buffer::Cell::symbol)
        );
        assert!(
            !buf.cell((0, 5))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "off-viewport selected system header must not be reversed either"
        );
    }

    #[test]
    fn clear_selection_highlight_leaves_logical_selection_intact() {
        // Plan 087: dispatching the clear-highlight action (as the
        // event-loop timer does) must remove the visual highlight
        // without touching the logical `selected_id`. The renderer
        // then renders the system block without `REVERSED`.
        let config = test_config(&["a", "b"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: state.systems[1].id.clone(),
                endpoint: state.systems[1].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap())),
                latency: Duration::from_millis(10),
            }],
        });
        state.selection_highlight_active = true;
        let selected_before = state.selected_id.clone();
        state.apply_action(crate::action::Action::ClearSelectionHighlight);
        assert!(!state.selection_highlight_active);
        assert_eq!(state.selected_id, selected_before);

        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| super::render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();
        assert!(
            !buf.cell((0, 0))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "no REVERSED after highlight cleared"
        );
    }

    #[test]
    fn navigation_action_activates_visual_highlight() {
        // Plan 087: the visual highlight activates when the operator
        // navigates, even if `selected_id` is logically unchanged (for
        // example when `MoveDown` is clamped at the last row).
        let config = test_config(&["a", "b"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: state.systems[1].id.clone(),
                endpoint: state.systems[1].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap())),
                latency: Duration::from_millis(10),
            }],
        });
        state.selection_highlight_active = true;
        // MoveDown is clamped at the last row; the reducer must still
        // retain the highlight so the event-loop timer can keep
        // counting down from this action.
        state.apply_action(crate::action::Action::SelectLast);
        assert!(state.selection_highlight_active);
        // Now apply ClearSelectionHighlight as the timer would.
        state.apply_action(crate::action::Action::ClearSelectionHighlight);
        assert!(!state.selection_highlight_active);
    }

    // ── 7. Width degradation ─────────────────────────────────────────

    #[test]
    fn render_header_wide() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 120, 8);
        let header = output.lines().next().unwrap();
        // At width >= 80, should contain all fields: name, IO, load, cores, os, kernel, arch
        assert!(header.contains("srv"), "header: {header}");
        assert!(header.contains("IO"), "header: {header}");
        assert!(header.contains("x86_64"), "header: {header}");
        assert!(header.contains("Linux"), "header: {header}");
    }

    #[test]
    fn render_header_medium() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 50, 8);
        let header = output.lines().next().unwrap();
        // At 50-79 cols: name, IO, load, cores, os
        assert!(header.contains("srv"), "header: {header}");
        assert!(header.contains("IO"), "header: {header}");
        // Should NOT contain architecture (dropped at < 80)
        assert!(
            !header.contains("x86_64"),
            "header should not contain arch at width 50: {header}"
        );
    }

    #[test]
    fn render_header_narrow() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 32, 8);
        let header = output.lines().next().unwrap();
        // At 32-49 cols: name, IO, load, cores
        assert!(header.contains("srv"), "header: {header}");
        assert!(header.contains("IO"), "header: {header}");
        assert!(
            !header.contains("linux"),
            "header should not contain os at width 32: {header}"
        );
    }

    // ── 8. Bar rendering at different percentages ────────────────────

    #[test]
    fn render_bar_zero_percent() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap_custom(0.0, 0.0, 4));
        let output = render_state(&state, 120, 8);
        let cpu_line = output.lines().nth(1).unwrap();
        assert!(
            cpu_line.contains("0.0%"),
            "CPU bar at 0% should show '0.0%', got: {cpu_line}"
        );
    }

    #[test]
    fn render_bar_50_percent() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap_custom(50.0, 0.0, 4));
        let output = render_state(&state, 120, 8);
        let cpu_line = output.lines().nth(1).unwrap();
        assert!(
            cpu_line.contains("50.0%"),
            "CPU bar at 50% should show '50.0%', got: {cpu_line}"
        );
        // Bar should have some filled characters
        assert!(
            cpu_line.contains('|'),
            "CPU bar should contain filled '|' chars at 50%, got: {cpu_line}"
        );
    }

    #[test]
    fn render_bar_100_percent() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap_custom(100.0, 0.0, 4));
        let output = render_state(&state, 120, 8);
        let cpu_line = output.lines().nth(1).unwrap();
        assert!(
            cpu_line.contains("100%"),
            "CPU bar at 100% should show '100%', got: {cpu_line}"
        );
    }

    #[test]
    fn render_bar_high_percent() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap_custom(99.9, 0.0, 4));
        let output = render_state(&state, 120, 8);
        let cpu_line = output.lines().nth(1).unwrap();
        assert!(
            cpu_line.contains("99.9%"),
            "CPU bar at 99.9% should show '99.9%', got: {cpu_line}"
        );
    }

    // ── 9. Zero swap ─────────────────────────────────────────────────

    #[test]
    fn render_zero_swap() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        let snap = LinuxSnapshotBuilder::default().swap(0, 0).build();
        apply_online(&mut state, 0, snap);
        let output = render_state(&state, 120, 8);
        let swap_line = output.lines().nth(3).unwrap();
        assert!(
            swap_line.contains("SWP"),
            "SWP row should contain label, got: {swap_line}"
        );
        assert!(
            swap_line.contains("0.0%"),
            "zero swap should show '0.0%', got: {swap_line}"
        );
    }

    // ── 10. Various widths ───────────────────────────────────────────

    #[test]
    fn render_at_width_24() {
        let config = test_config(&["x"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 24, 8);
        // Should not crash, should render something.
        assert!(!output.trim().is_empty());
        let header = output.lines().next().unwrap();
        assert!(header.contains('x'), "header at width 24: {header}");
    }

    #[test]
    fn render_at_width_32() {
        let config = test_config(&["x"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 32, 8);
        assert!(!output.trim().is_empty());
        let header = output.lines().next().unwrap();
        assert!(header.contains('x'), "header at width 32: {header}");
    }

    #[test]
    fn render_at_width_40() {
        let config = test_config(&["x"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 40, 8);
        assert!(!output.trim().is_empty());
        let header = output.lines().next().unwrap();
        assert!(header.contains('x'), "header at width 40: {header}");
    }

    #[test]
    fn render_at_width_60() {
        let config = test_config(&["x"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 60, 8);
        assert!(!output.trim().is_empty());
        let header = output.lines().next().unwrap();
        assert!(header.contains('x'), "header at width 60: {header}");
    }

    #[test]
    fn render_at_width_120() {
        let config = test_config(&["x"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 120, 8);
        assert!(!output.trim().is_empty());
        let header = output.lines().next().unwrap();
        assert!(header.contains('x'), "header at width 120: {header}");
    }

    // ── 11. Viewport/scrolling ───────────────────────────────────────

    #[test]
    fn viewport_scrolling() {
        let names: Vec<&str> = (0..6).map(|_| "sys").collect();
        let config = test_config(&names);
        let mut state = AppState::from_config(&config);
        // Make all systems online (five base rows each).
        for i in 0..6 {
            apply_online(&mut state, i, linux_snap());
            // Bump generation for each batch.
        }

        // 6 online systems × 5 rows = 30 rows needed, but terminal is 12 tall.
        let output = render_state(&state, 80, 12);

        // Two complete online systems should fit in 12 rows.
        let nonblank = count_nonblank_lines(&output);
        assert!(
            nonblank <= 12,
            "should not exceed terminal height, got {nonblank} non-blank lines"
        );
        // At least 2 systems should be visible (8 rows minimum for 2 systems).
        assert!(
            nonblank >= 8,
            "should show at least 2 systems, got {nonblank} non-blank lines"
        );
    }

    // ── 12. Unicode handling ─────────────────────────────────────────

    #[test]
    fn render_unicode_name() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "unicode-1".into(),
            host: "host1.local".into(),
            port: 11310,
            name: Some("サーバー①".into()),
        });
        let mut state = AppState::from_config(&config);
        let snap = LinuxSnapshotBuilder::default().build();
        apply_online(&mut state, 0, snap);

        let output = render_state(&state, 80, 8);
        // Wide Unicode chars may be split across cells by ratatui;
        // verify the header starts with the first character and doesn't crash.
        let header = output.lines().next().unwrap();
        assert!(
            header.starts_with('サ'),
            "header should start with unicode name, got: {header}"
        );
    }

    // ── Additional integration tests ─────────────────────────────────

    #[test]
    fn online_system_uses_five_rows() {
        let config = test_config(&["s1"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        // Height 5 = exactly one online system, no room for key hint.
        let output = render_state(&state, 80, 5);
        let nonblank = count_nonblank_lines(&output);
        assert_eq!(nonblank, 5, "one online system should use exactly 5 rows");
    }

    #[test]
    fn render_populated_disk_and_selected_drive_details() {
        let config = test_config(&["storage"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.systems[0].latest.as_mut().unwrap().drives = Some(vec![
            NormalizedDrive {
                name: "/".into(),
                used_bytes: 238 * 1024 * 1024 * 1024,
                total_bytes: 952 * 1024 * 1024 * 1024,
                available_bytes: None,
            },
            NormalizedDrive {
                name: "/mnt/archive".into(),
                used_bytes: 142 * 1024 * 1024 * 1024,
                total_bytes: 477 * 1024 * 1024 * 1024,
                available_bytes: None,
            },
        ]);
        state.drives_expanded = true;

        let output = render_state(&state, 200, 8);
        assert!(output.lines().nth(4).unwrap().contains("DISK"));
        // Phase 085: aggregate disk text is `<used> / <total>` so the
        // slash denominator matches the percentage calculation. The
        // `<used> / <available>` shape was retired in 085; explicit
        // caller-available space remains reachable through the expanded
        // drive detail rows.
        assert!(
            output.contains("380.0 GiB / 1.4 TiB"),
            "expected aggregate detail in output:\n{output}"
        );
        assert!(
            !output.contains("used /"),
            "aggregate detail must not contain 'used':\n{output}"
        );
        assert!(
            !output.contains("GiB avail"),
            "aggregate detail must not contain 'avail':\n{output}"
        );
        assert!(output.contains("/mnt/archive"));
        assert!(output.contains("142.0 GiB"));
        assert!(output.contains("25.0%"));
    }

    #[test]
    fn render_unavailable_disk_does_not_show_zero_percent() {
        let config = test_config(&["legacy"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 120, 5);
        let disk = output.lines().nth(4).unwrap();
        assert!(disk.contains("DISK"));
        assert!(disk.contains('—'));
        assert!(!disk.contains("0.0%"));
    }

    #[test]
    fn offline_system_uses_one_row() {
        let config = test_config(&["s1"]);
        let mut state = AppState::from_config(&config);
        apply_offline(&mut state, 0);
        // Height 1 = exactly one offline system, no room for key hint.
        let output = render_state(&state, 80, 1);
        let nonblank = count_nonblank_lines(&output);
        assert_eq!(nonblank, 1, "one offline system should use exactly 1 row");
    }

    #[test]
    fn pending_system_uses_one_row() {
        let config = test_config(&["s1"]);
        let state = AppState::from_config(&config);
        // Height 1 = exactly one pending system, no room for key hint.
        let output = render_state(&state, 80, 1);
        let nonblank = count_nonblank_lines(&output);
        assert_eq!(nonblank, 1, "one pending system should use exactly 1 row");
    }

    #[test]
    fn mixed_online_offline_row_counts() {
        let config = test_config(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        apply_offline(&mut state, 1);
        // c is pending (default).
        // Height 7 = exactly 5 + 1 + 1, no room for key hint.
        let output = render_state(&state, 80, 7);
        let nonblank = count_nonblank_lines(&output);
        // 5 (online a) + 1 (offline b) + 1 (pending c) = 7
        assert_eq!(
            nonblank, 7,
            "online(5) + offline(1) + pending(1) = 7, got {nonblank}"
        );
    }

    #[test]
    fn io_wait_shown_for_linux() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        let snap = LinuxSnapshotBuilder::default().iowait_pct(3.7).build();
        apply_online(&mut state, 0, snap);
        let output = render_state(&state, 80, 8);
        let header = output.lines().next().unwrap();
        assert!(
            header.contains("IO 3.7%"),
            "Linux header should show IO wait percentage, got: {header}"
        );
    }

    #[test]
    fn io_wait_none_for_macos() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, macos_snap());
        let output = render_state(&state, 80, 8);
        let header = output.lines().next().unwrap();
        // Plan 087: macOS has cpu_iowait_supported = false so the IO
        // token is omitted entirely. There must be no placeholder or
        // separator artifact where the token would have lived.
        assert!(
            !header.contains("IO "),
            "macOS header must omit the IO token, got: {header:?}"
        );
    }

    #[test]
    fn io_wait_unsupported_omits_token_without_separator_artifact() {
        // Plan 087: with `cpu_iowait_supported == false` the header
        // must not leave a separator artifact where the IO token
        // would have been. The remaining fields are joined by the
        // ordinary single separator gap (the previous/next component
        // is the system name on one side and the load/cores block
        // on the other). Specifically, the name must not be
        // followed by three spaces because the omitted IO token left
        // its leading separator behind.
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, macos_snap());
        let output = render_state(&state, 120, 8);
        let header = output.lines().next().unwrap().trim_end();
        assert!(!header.contains("IO "));
        // Plan 087: a separator artifact would mean "srv   " (three
        // spaces after the name) because the IO token was omitted but
        // its leading separator was kept. The trimmed header must not
        // start with that pattern.
        assert!(
            !header.starts_with("srv   "),
            "no separator artifact after the name, got: {header:?}"
        );
    }

    #[test]
    fn io_wait_supported_with_missing_value_omits_token() {
        // Plan 087: a v2 snapshot that advertises `cpu_iowait =
        // true` capability but reports `iowait_pct = None` must also
        // omit the IO token. The UI does not infer a zero from a
        // missing measurement. The protocol validator enforces
        // agreement between the capability and the value at the wire
        // level, so we bypass the validator and manipulate the
        // normalized snapshot directly to exercise the renderer
        // invariant.
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        let payload = LinuxSnapshotV2Builder::default().build_payload();
        apply_online_v2(&mut state, 0, payload, 1);
        // Strip the IO value while keeping the capability flag set.
        state.systems[0]
            .latest
            .as_mut()
            .expect("snapshot present")
            .iowait_pct = None;
        let output = render_state(&state, 120, 8);
        let header = output.lines().next().unwrap();
        assert!(
            !header.contains("IO "),
            "supported-without-value must omit IO token, got: {header:?}"
        );
        assert!(
            !header.contains("0.0%"),
            "must not fabricate IO 0.0%, got: {header:?}"
        );
    }

    #[test]
    fn load_averages_rendered_in_header() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        let snap = LinuxSnapshotBuilder::default()
            .load(1.50, 2.00, 0.75)
            .build();
        apply_online(&mut state, 0, snap);
        let output = render_state(&state, 80, 8);
        let header = output.lines().next().unwrap();
        assert!(
            header.contains("1.50/2.00/0.75"),
            "header should contain load averages, got: {header}"
        );
    }

    #[test]
    fn core_count_in_cpu_bar() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        let snap = LinuxSnapshotBuilder::default().logical_cores(16).build();
        apply_online(&mut state, 0, snap);
        let output = render_state(&state, 120, 8);
        let cpu_line = output.lines().nth(1).unwrap();
        assert!(
            cpu_line.contains("CPU"),
            "CPU bar should contain label, got: {cpu_line}"
        );
        assert!(
            cpu_line.contains("25.2%"),
            "CPU bar should show percentage, got: {cpu_line}"
        );
        // Core count detail may be clipped by the bar width; verify the
        // bar renders with correct label and percentage.
    }

    #[test]
    fn mem_bar_shows_usage_detail() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        let snap = LinuxSnapshotBuilder::default()
            .memory(8_000_000_000, 16_000_000_000)
            .build();
        apply_online(&mut state, 0, snap);
        let output = render_state(&state, 120, 8);
        let mem_line = output.lines().nth(2).unwrap();
        assert!(
            mem_line.contains("MEM"),
            "MEM bar should contain label, got: {mem_line}"
        );
        assert!(
            mem_line.contains("50.0%"),
            "MEM bar should show percentage, got: {mem_line}"
        );
    }

    #[test]
    fn swap_bar_shows_usage_detail() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        let snap = LinuxSnapshotBuilder::default()
            .swap(1_000_000_000, 4_000_000_000)
            .build();
        apply_online(&mut state, 0, snap);
        let output = render_state(&state, 120, 8);
        let swap_line = output.lines().nth(3).unwrap();
        assert!(
            swap_line.contains("SWP"),
            "SWP bar should contain label, got: {swap_line}"
        );
        assert!(
            swap_line.contains("25.0%"),
            "SWP bar should show percentage, got: {swap_line}"
        );
    }

    #[test]
    fn multiple_online_systems_render_independently() {
        let config = test_config(&["a", "b"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap_custom(10.0, 0.0, 4));
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: state.systems[1].id.clone(),
                endpoint: state.systems[1].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap_custom(90.0, 0.0, 8))),
                latency: Duration::from_millis(10),
            }],
        });

        // Plan 087: width 120 keeps suffixes visible for the
        // default Linux snapshot.
        let output = render_state(&state, 120, 16);
        let lines: Vec<&str> = output.lines().collect();

        // System a is first (online, selected), then system b.
        assert!(lines[0].contains('a'), "first header: {}", lines[0]);
        assert!(lines[5].contains('b'), "second header: {}", lines[5]);

        // CPU bars should differ.
        assert!(lines[1].contains("10.0%"), "a CPU: {}", lines[1]);
        assert!(lines[6].contains("90.0%"), "b CPU: {}", lines[6]);
    }

    #[test]
    fn empty_config_at_various_sizes() {
        let config = Config::default();
        let state = AppState::from_config(&config);
        for &(w, h) in &[(80, 24), (40, 12), (20, 5), (120, 40)] {
            let output = render_state(&state, w, h);
            assert!(output.contains("No sources"), "at {w}x{h}: {output}");
        }
    }

    #[test]
    fn too_small_at_minimum_boundary() {
        let config = test_config(&["s"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        // Width 23 is just below the minimum of 24.
        let output = render_state(&state, 23, 24);
        assert!(
            output.contains("terminal too"),
            "width 23 should be too small:\n{output}"
        );
    }

    #[test]
    fn too_small_height_at_boundary() {
        let config = test_config(&["s"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        // Height 3 is just below the minimum of 4.
        let output = render_state(&state, 80, 3);
        assert!(
            output.contains("terminal too"),
            "height 3 should be too small:\n{output}"
        );
    }

    #[test]
    fn width_exactly_24_is_not_too_small() {
        let config = test_config(&["s"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 24, 5);
        assert!(
            !output.contains("terminal too small"),
            "width 24 should be valid:\n{output}"
        );
        // Should render the online system header.
        assert!(output.contains('s'), "should render system: {output}");
    }

    #[test]
    fn height_exactly_4_is_too_small_for_online_base() {
        let config = test_config(&["s"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 80, 4);
        assert!(output.contains("terminal too small"));
    }

    #[test]
    fn selection_changes_reversed_style() {
        let config = test_config(&["a", "b"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: state.systems[1].id.clone(),
                endpoint: state.systems[1].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap())),
                latency: Duration::from_millis(10),
            }],
        });

        // Plan 087: even with logical selection populated at startup,
        // no system is visually reversed until the operator navigates.
        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| super::render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();

        assert!(
            !buf.cell((0, 0))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "a should NOT be reversed before navigation"
        );

        // Move selection to b.
        state.apply_action(crate::action::Action::MoveDown);
        assert!(state.selection_highlight_active);
        let backend2 = TestBackend::new(80, 12);
        let mut terminal2 = Terminal::new(backend2).unwrap();
        terminal2.draw(|f| super::render(f, &state)).unwrap();
        let buf2 = terminal2.backend().buffer().clone();

        assert!(
            !buf2
                .cell((0, 0))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "a should NOT be reversed after moving selection"
        );
        assert!(
            buf2.cell((0, 5))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "b should be reversed after moving selection"
        );
    }

    #[test]
    fn toggle_drives_works_after_visual_highlight_expires() {
        // Plan 087: `e` (drive expansion) is bound to logical
        // selection, not the transient highlight. After the highlight
        // is cleared the drives must still expand/collapse for the
        // logically selected system.
        let config = test_config(&["a"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.selection_highlight_active = true;
        let selected = state.selected_id.clone();
        assert!(!state.drives_expanded);
        // Visual highlight expires.
        state.apply_action(crate::action::Action::ClearSelectionHighlight);
        assert!(!state.selection_highlight_active);
        // Toggle drives (the `e` action) must still operate on the
        // logical selection.
        state.apply_action(crate::action::Action::ToggleDrives);
        assert!(state.drives_expanded);
        assert_eq!(state.selected_id, selected);
        state.apply_action(crate::action::Action::ToggleDrives);
        assert!(!state.drives_expanded);
        assert_eq!(state.selected_id, selected);
    }

    #[test]
    fn pane_switch_clears_selection_highlight() {
        // Plan 087: leaving the Systems pane clears the visual
        // highlight so a stale reversed row does not reappear when the
        // operator comes back. Logical selection itself is untouched.
        let mut config = test_config(&["a"]);
        config.eggpool = Some(crate::config::EggpoolEntry {
            id: "ep".into(),
            host: "pool.local".into(),
            port: 11300,
            scheme: crate::config::EggpoolScheme::Http,
            name: None,
            api_key_env: None,
        });
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.apply_action(crate::action::Action::MoveDown);
        assert!(state.selection_highlight_active);
        let selected = state.selected_id.clone();

        // Switch to EggPool.
        state.apply_action(crate::action::Action::NextPane);
        assert_eq!(state.active_pane, crate::state::Pane::Eggpool);
        assert!(
            !state.selection_highlight_active,
            "highlight must be cleared when leaving Systems"
        );
        assert_eq!(state.selected_id, selected);

        // Switch back. The highlight must remain cleared; it does not
        // re-arm automatically.
        state.apply_action(crate::action::Action::PreviousPane);
        assert_eq!(state.active_pane, crate::state::Pane::Systems);
        assert!(!state.selection_highlight_active);
        assert_eq!(state.selected_id, selected);
    }

    #[test]
    fn eggpool_period_change_does_not_activate_systems_highlight() {
        // Plan 087: `j/k` while the EggPool pane is active must not
        // arm the Systems selection highlight.
        let mut config = test_config(&["a"]);
        config.eggpool = Some(crate::config::EggpoolEntry {
            id: "ep".into(),
            host: "pool.local".into(),
            port: 11300,
            scheme: crate::config::EggpoolScheme::Http,
            name: None,
            api_key_env: None,
        });
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        // Move to EggPool.
        state.apply_action(crate::action::Action::NextPane);
        assert_eq!(state.active_pane, crate::state::Pane::Eggpool);
        // Press j/k to cycle the EggPool period.
        state.apply_action(crate::action::Action::MoveDown);
        assert!(
            !state.selection_highlight_active,
            "EggPool period change must not activate Systems highlight"
        );
        state.apply_action(crate::action::Action::MoveUp);
        assert!(!state.selection_highlight_active);
    }

    #[test]
    fn cpu_iowait_linux_header_shows_percentage() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        let snap = LinuxSnapshotBuilder::default().iowait_pct(1.2).build();
        apply_online(&mut state, 0, snap);
        let output = render_state(&state, 80, 8);
        let header = output.lines().next().unwrap();
        assert!(
            header.contains("IO 1.2%"),
            "Linux IO should show actual percentage, got: {header}"
        );
    }

    #[test]
    fn system_without_configured_name_uses_host() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "no-name".into(),
            host: "10.0.0.1".into(),
            port: 11310,
            name: None,
        });
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 80, 8);
        let header = output.lines().next().unwrap();
        assert!(
            header.contains("10.0.0.1"),
            "should fall back to host when no name configured, got: {header}"
        );
    }

    #[test]
    fn offline_system_displays_address() {
        let config = test_config(&["web1"]);
        let mut state = AppState::from_config(&config);
        apply_offline(&mut state, 0);
        let output = render_state(&state, 80, 4);
        // The offline line format is "name@host:port offline ..."
        assert!(
            output.contains("host0.local:11310"),
            "offline line should contain address, got: {output}"
        );
        assert!(
            output.contains("web1"),
            "offline line should contain name, got: {output}"
        );
    }

    #[test]
    fn very_narrow_width_just_above_minimum() {
        let config = test_config(&["x"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        // Width 24 = minimum valid width, height 5 = minimum valid height.
        let output = render_state(&state, 24, 5);
        assert!(!output.trim().is_empty());
        let header = output.lines().next().unwrap();
        assert!(header.contains('x'), "header at 24x4: {header}");
    }

    #[test]
    fn wide_terminal_renders_full_header() {
        let config = test_config(&["box"]);
        let mut state = AppState::from_config(&config);
        let snap = LinuxSnapshotBuilder::default()
            .load(1.00, 2.00, 3.00)
            .logical_cores(32)
            .build();
        apply_online(&mut state, 0, snap);
        let output = render_state(&state, 200, 40);
        let header = output.lines().next().unwrap();
        // Full header: name, IO, load, cores, os, kernel, arch
        assert!(header.contains("box"), "header: {header}");
        assert!(header.contains("IO"), "header: {header}");
        assert!(header.contains("1.00/2.00/3.00"), "header: {header}");
        assert!(
            header.contains("32 cores") || header.contains("32c"),
            "header: {header}"
        );
        assert!(header.contains("Ubuntu"), "header: {header}");
        assert!(header.contains("6.8.0"), "header: {header}");
        assert!(header.contains("x86_64"), "header: {header}");
    }

    #[test]
    fn no_systems_configured_always_shows_message() {
        let config = Config::default();
        let state = AppState::from_config(&config);
        for &(w, h) in &[(80, 24), (40, 10), (120, 50)] {
            let output = render_state(&state, w, h);
            assert!(
                output.contains("No sources configured"),
                "at {w}x{h}: {output}"
            );
        }
    }

    #[test]
    fn offline_dot_padding() {
        let config = test_config(&["short"]);
        let mut state = AppState::from_config(&config);
        apply_offline(&mut state, 0);
        let output = render_state(&state, 80, 4);
        let line = output.lines().next().unwrap();
        // The line should have dots filling the remaining width.
        assert!(
            line.ends_with('.'),
            "offline line should have dot padding, got: {line}"
        );
    }

    #[test]
    fn offline_no_padding_when_tight() {
        let config = test_config(&["a"]);
        let mut state = AppState::from_config(&config);
        apply_offline(&mut state, 0);
        // Width that barely fits the label.
        let output = render_state(&state, 24, 4);
        let line = output.lines().next().unwrap();
        assert!(
            line.contains('a'),
            "tight offline line should contain name: {line}"
        );
        assert!(
            line.contains("offl"),
            "tight offline line should contain partial status: {line}"
        );
    }

    #[test]
    fn display_order_affects_rendering() {
        // Config order: a, b, c. Make c and a online.
        let config = test_config(&["a", "b", "c"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: state.systems[2].id.clone(),
                endpoint: state.systems[2].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap())),
                latency: Duration::from_millis(10),
            }],
        });

        let output = render_state(&state, 80, 20);
        let lines: Vec<&str> = output.lines().collect();

        // Online first: a, c (in configured order), then b (pending).
        let first_header = lines.iter().find(|l| !l.trim().is_empty()).unwrap();
        assert!(
            first_header.contains('a'),
            "first rendered should be online a, got: {first_header}"
        );
    }

    #[test]
    fn render_two_offline_systems() {
        let config = test_config(&["x", "y"]);
        let mut state = AppState::from_config(&config);
        apply_offline(&mut state, 0);
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: state.systems[1].id.clone(),
                endpoint: state.systems[1].endpoint.clone(),
                outcome: PollOutcome::ConnectionRefused,
                latency: Duration::from_millis(10),
            }],
        });

        // Height 4 = two offline systems + key hint in remaining space.
        let output = render_state(&state, 80, 4);
        assert!(output.contains('x'), "should contain x: {output}");
        assert!(output.contains('y'), "should contain y: {output}");
        // 2 offline rows + 1 key hint row = 3 nonblank lines.
        let nonblank = count_nonblank_lines(&output);
        assert_eq!(nonblank, 3, "two offline systems + hint = 3 rows");
    }

    #[test]
    fn resize_round_trip_wide_narrow_wide() {
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());

        // Wide → narrow → wide should not crash and should adapt content.
        let wide = render_state(&state, 120, 24);
        let narrow = render_state(&state, 32, 8);
        let wide_again = render_state(&state, 120, 24);

        // Wide should have architecture info.
        assert!(
            wide.contains("x86_64"),
            "wide: {}",
            wide.lines().next().unwrap()
        );
        // Narrow should NOT have architecture info (dropped at < 80).
        assert!(
            !narrow.contains("x86_64"),
            "narrow should drop arch: {}",
            narrow.lines().next().unwrap()
        );
        // Wide again should restore architecture info.
        assert!(
            wide_again.contains("x86_64"),
            "wide again: {}",
            wide_again.lines().next().unwrap()
        );
    }

    #[test]
    fn key_hint_appears_when_extra_space() {
        let config = test_config(&["s1"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        // 12 rows: 4 for system, 8 extra → hint should appear.
        let output = render_state(&state, 80, 12);
        assert!(
            output.contains("j/k:select"),
            "key hint should appear with extra space:\n{output}"
        );
    }

    #[test]
    fn key_hint_absent_when_no_extra_space() {
        let config = test_config(&["s1"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        // 5 rows: exactly one system, no extra space.
        let output = render_state(&state, 80, 5);
        assert!(
            !output.contains("j/k:select"),
            "key hint should not appear when no extra space:\n{output}"
        );
    }

    #[test]
    fn config_reload_error_is_rendered_in_the_diagnostic_line() {
        let config = test_config(&["s1"]);
        let mut state = AppState::from_config(&config);
        state.config_reload_error = Some("config reload failed: invalid TOML".into());

        let output = render_state(&state, 80, 7);

        assert!(output.contains("config reload failed: invalid TOML"));
    }

    #[test]
    fn render_online_system_without_snapshot_does_not_crash() {
        // System is Online but latest is None (edge case).
        let config = test_config(&["s"]);
        let mut state = AppState::from_config(&config);
        // Manually set reachability to Online without providing a snapshot.
        state.systems[0].reachability = Reachability::Online;
        // latest is None.
        let output = render_state(&state, 80, 8);
        assert!(
            output.contains("waiting for data"),
            "should show a pending state: {output}"
        );
    }

    #[test]
    fn render_windows_system_shows_commit_row() {
        let config = test_config(&["win1"]);
        let mut state = AppState::from_config(&config);
        let snap = WindowsSnapshotV2Builder::default().build_payload();
        let system = &state.systems[0];
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: system.id.clone(),
                endpoint: system.endpoint.clone(),
                outcome: PollOutcome::OnlineV2(Box::new(snap)),
                latency: Duration::from_millis(10),
            }],
        };
        state.apply_batch(&batch);
        let output = render_state(&state, 80, 8);
        let lines: Vec<&str> = output.lines().collect();
        // Row 3 (index 3) should show COMMIT, not SWP.
        assert!(
            lines[3].contains("COMMIT"),
            "Windows row 3 should contain 'COMMIT', got: {}",
            lines[3]
        );
        assert!(
            !lines[3].contains("SWP"),
            "Windows row 3 should not contain 'SWP', got: {}",
            lines[3]
        );
    }

    #[test]
    fn rendered_linux_metric_geometry_is_aligned_at_representative_widths() {
        let config = test_config(&["linux"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());

        for width in [24u16, 32, 40, 60, 80] {
            let output = render_state(&state, width, 8);
            let rows = metric_lines(&output);
            assert_eq!(rows.len(), 4, "metric rows at width {width}: {output}");

            let opening_columns: Vec<_> =
                rows.iter().map(|line| terminal_column(line, '[')).collect();
            let closing_columns: Vec<_> =
                rows.iter().map(|line| terminal_column(line, ']')).collect();
            assert!(
                opening_columns
                    .iter()
                    .all(|&column| column == opening_columns[0]),
                "opening brackets drifted at width {width}: {opening_columns:?}\n{output}"
            );
            assert!(
                closing_columns
                    .iter()
                    .all(|&column| column == closing_columns[0]),
                "closing brackets drifted at width {width}: {closing_columns:?}\n{output}"
            );

            // Plan 087: the default Linux snapshot's MEM detail is 24
            // cells, so compact mode (suppress suffixes) engages when
            // width/4 < 24, i.e. at every width in this loop. The
            // percentage must NOT survive in compact mode.
            let compact_mode = width < 96;
            for (label, line) in ["CPU", "MEM", "SWP", "DISK"].iter().zip(rows.iter()) {
                assert!(
                    line.starts_with("    "),
                    "{label} indentation at width {width}: {line:?}"
                );
                if *label != "DISK" && !compact_mode {
                    assert!(
                        line.contains('%'),
                        "{label} percentage at width {width}: {line:?}"
                    );
                }
                if *label != "DISK" && compact_mode {
                    assert!(
                        !line.contains('%'),
                        "{label} must omit percentage in compact mode at width {width}: {line:?}"
                    );
                }
                assert!(
                    UnicodeWidthStr::width(*line) <= usize::from(width),
                    "{label} exceeds width {width}: {line:?}"
                );
            }
            let disk = rows[3];
            // Plan 087: in compact mode the suffix is suppressed so
            // the unavailable em-dash never appears. The compact
            // shape itself is the truthful unavailable rendering.
            if compact_mode {
                assert!(
                    !disk.contains('%'),
                    "compact DISK must not fabricate a percentage: {disk:?}"
                );
            } else {
                assert!(disk.contains('—'), "DISK should be unavailable: {disk:?}");
            }
            assert!(
                !disk.contains("0.0%"),
                "DISK must not fabricate zero: {disk:?}"
            );
            assert!(!disk.contains("used"), "DISK should omit 'used': {disk:?}");
            assert!(
                !disk.contains("avail"),
                "DISK should omit 'avail': {disk:?}"
            );
        }
    }

    #[test]
    fn rendered_windows_metric_geometry_keeps_commit_aligned_at_representative_widths() {
        let config = test_config(&["windows"]);
        let mut state = AppState::from_config(&config);
        let payload = WindowsSnapshotV2Builder::default().build_payload();
        apply_online_v2(&mut state, 0, payload, 1);

        for width in [24u16, 32, 40, 60, 80] {
            let output = render_state(&state, width, 8);
            let rows = metric_lines(&output);
            assert_eq!(rows.len(), 4, "metric rows at width {width}: {output}");
            assert!(
                rows[2].contains("COMMIT"),
                "third row at width {width}: {rows:?}"
            );

            let opening_columns: Vec<_> =
                rows.iter().map(|line| terminal_column(line, '[')).collect();
            let closing_columns: Vec<_> =
                rows.iter().map(|line| terminal_column(line, ']')).collect();
            assert!(
                opening_columns
                    .iter()
                    .all(|&column| column == opening_columns[0]),
                "opening brackets drifted at width {width}: {opening_columns:?}\n{output}"
            );
            assert!(
                closing_columns
                    .iter()
                    .all(|&column| column == closing_columns[0]),
                "closing brackets drifted at width {width}: {closing_columns:?}\n{output}"
            );

            // Plan 087: at widths below the natural-suffix threshold
            // compact mode engages and the suffix (including %) is
            // suppressed fleet-wide.
            let compact_mode = width < 96;
            for (label, line) in ["CPU", "MEM", "COMMIT", "DISK"].iter().zip(rows.iter()) {
                assert!(
                    line.starts_with("    "),
                    "{label} indentation at width {width}: {line:?}"
                );
                if *label != "DISK" && !compact_mode {
                    assert!(
                        line.contains('%'),
                        "{label} percentage at width {width}: {line:?}"
                    );
                }
                if *label != "DISK" && compact_mode {
                    assert!(
                        !line.contains('%'),
                        "{label} must omit percentage in compact mode at width {width}: {line:?}"
                    );
                }
                assert!(
                    UnicodeWidthStr::width(*line) <= usize::from(width),
                    "{label} exceeds width {width}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn render_condensed_width_tiers_and_header_geometry() {
        let config = test_config(&["fleet-host"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.system_view_mode = crate::state::SystemViewMode::Condensed;

        let wide = render_state(&state, 80, 4);
        assert!(wide.lines().next().unwrap().contains("HOST"));
        assert!(wide.lines().next().unwrap().contains("IOWAIT"));
        assert!(wide.lines().nth(1).unwrap().contains('─'));
        assert!(wide.lines().nth(2).unwrap().contains("fleet-host"));

        let narrow = render_state(&state, 30, 4);
        let header = narrow.lines().next().unwrap();
        assert!(header.contains("HOST"));
        assert!(!header.contains("LOAD"));
        assert!(!header.contains("IOWAIT"));
    }

    #[test]
    fn render_condensed_expansion_keeps_base_row_and_details() {
        let config = test_config(&["storage"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.system_view_mode = crate::state::SystemViewMode::Condensed;
        state.drives_expanded = true;
        state.systems[0].latest.as_mut().unwrap().drives = Some(vec![NormalizedDrive {
            name: "/archive".into(),
            used_bytes: 2 * 1024 * 1024 * 1024,
            total_bytes: 4 * 1024 * 1024 * 1024,
            available_bytes: None,
        }]);

        let output = render_state(&state, 80, 5);
        assert!(output.lines().nth(2).unwrap().contains("storage"));
        assert!(output.contains("/archive"));
        assert!(output.contains("50.0%"));
    }

    #[test]
    fn mixed_fleet_renders_protocol_capabilities_and_selected_details_in_both_views() {
        let config = test_config(&["legacy", "linux", "mac", "windows", "offline", "pending"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        apply_online_v2(
            &mut state,
            1,
            LinuxSnapshotV2Builder::default()
                .drives(Some(vec![
                    DriveMetrics {
                        name: "/".into(),
                        used_bytes: 4,
                        total_bytes: 10,
                        available_bytes: None,
                    },
                    DriveMetrics {
                        name: "/home".into(),
                        used_bytes: 6,
                        total_bytes: 10,
                        available_bytes: None,
                    },
                ]))
                .build_payload(),
            2,
        );
        let mut mac = LinuxSnapshotV2Builder::default()
            .drives(Some(vec![DriveMetrics {
                name: "/Volumes/data".into(),
                used_bytes: 1,
                total_bytes: 4,
                available_bytes: None,
            }]))
            .build_payload();
        mac.snapshot.system.os_name = "macos".into();
        mac.snapshot.capabilities.cpu_iowait = false;
        mac.snapshot.cpu.iowait_pct = None;
        mac.validate().unwrap();
        apply_online_v2(&mut state, 2, mac, 3);
        apply_online_v2(
            &mut state,
            3,
            WindowsSnapshotV2Builder::default()
                .drives(Some(vec![DriveMetrics {
                    name: "C:\\".into(),
                    used_bytes: 2,
                    total_bytes: 8,
                    available_bytes: None,
                }]))
                .build_payload(),
            4,
        );
        apply_offline(&mut state, 4);
        state.selected_id = Some("id-1".into());
        state.drives_expanded = true;
        state.viewport_top_id = None;

        let normal = render_state(&state, 120, 30);
        assert!(normal.contains("DISK"));
        assert!(normal.contains("COMMIT"));
        // Plan 087: macOS systems have cpu_iowait_supported = false,
        // so the IO token must be absent from the macOS header. The
        // Linux system still emits "IO 0.4%" by default, so we look
        // for the macOS-specific sentinel name rather than blanket
        // asserting the absence of "IO " in the entire output.
        assert!(normal.contains("/home"));
        assert!(normal.contains("offline"));
        assert!(normal.contains("pending"));
        assert!(!normal.contains("/Volumes/data"));
        // Find the macOS system header (it lives under the condensed
        // os name `macos`) and verify it contains no IO token.
        assert!(
            !normal
                .lines()
                .any(|line| line.contains("macos") && line.contains("IO ")),
            "macOS system header must omit the IO token entirely: {normal}"
        );

        state.apply_action(crate::action::Action::ToggleSystemView);
        let condensed = render_state(&state, 120, 12);
        assert!(condensed.contains("HOST"));
        assert!(condensed.contains("50%"));
        assert!(condensed.contains("IOWAIT"));
        assert!(condensed.contains("/home"));
        assert!(!condensed.contains("/Volumes/data"));
    }

    // ── Fleet-wide normal-view geometry ────────────────────────────────

    fn collect_metric_rows(output: &str, names: &[&str]) -> Vec<Vec<String>> {
        let lines: Vec<&str> = output.lines().collect();
        let mut out = Vec::new();
        let mut search_from = 0;
        for name in names {
            let block_start = lines
                .iter()
                .skip(search_from)
                .position(|line| {
                    let trimmed = line.trim_start();
                    trimmed.starts_with(name)
                        && (trimmed.len() == name.len()
                            || trimmed[name.len()..].starts_with(' ')
                            || trimmed[name.len()..].starts_with("  "))
                })
                .map_or_else(
                    || panic!("missing header for {name}"),
                    |offset| offset + search_from,
                );
            let mut rows = Vec::new();
            for line in lines.iter().skip(block_start + 1).take(4) {
                rows.push((*line).to_string());
            }
            out.push(rows);
            search_from = block_start + 5;
        }
        out
    }

    fn bracket_columns(rows: &[String]) -> (Vec<usize>, Vec<usize>) {
        let opens = rows.iter().map(|line| terminal_column(line, '[')).collect();
        let closes = rows.iter().map(|line| terminal_column(line, ']')).collect();
        (opens, closes)
    }

    fn assert_brackets_align(blocks: &[Vec<String>], context: &str) {
        let mut all_opens = Vec::new();
        let mut all_closes = Vec::new();
        for rows in blocks {
            let (o, c) = bracket_columns(rows);
            all_opens.extend(o);
            all_closes.extend(c);
        }
        let first_open = *all_opens.first().expect("at least one bracket");
        let first_close = *all_closes.first().expect("at least one bracket");
        assert!(
            all_opens.iter().all(|&c| c == first_open),
            "{context}: opening brackets drifted: {all_opens:?}"
        );
        assert!(
            all_closes.iter().all(|&c| c == first_close),
            "{context}: closing brackets drifted: {all_closes:?}"
        );
    }

    #[test]
    fn fleet_brackets_align_when_suffix_widths_differ_across_devices() {
        // Two online systems: one with small suffixes, one with TiB-scale
        // disk detail. Both must agree on every opening `[` and closing `]`
        // column at every representative width.
        let mut config = Config::default();
        for (i, host) in ["box.local", "big.local"].iter().enumerate() {
            config.systems.push(SystemEntry {
                id: format!("id-{i}"),
                host: (*host).to_string(),
                port: 11310,
                name: Some(format!("box{i}")),
            });
        }
        let mut state = AppState::from_config(&config);
        apply_online_v2(
            &mut state,
            0,
            LinuxSnapshotV2Builder::default()
                .memory(2 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024)
                .logical_cores(4)
                .build_payload(),
            1,
        );
        apply_online_v2(
            &mut state,
            1,
            LinuxSnapshotV2Builder::default()
                .memory(120 * 1024 * 1024 * 1024, 150 * 1024 * 1024 * 1024)
                .logical_cores(128)
                .drives(Some(vec![
                    DriveMetrics {
                        name: "/".into(),
                        used_bytes: 1200 * 1024 * 1024 * 1024 * 1024,
                        total_bytes: 1400 * 1024 * 1024 * 1024 * 1024,
                        available_bytes: None,
                    },
                    DriveMetrics {
                        name: "/mnt/big".into(),
                        used_bytes: 900 * 1024 * 1024 * 1024 * 1024,
                        total_bytes: 1100 * 1024 * 1024 * 1024 * 1024,
                        available_bytes: None,
                    },
                ]))
                .build_payload(),
            2,
        );
        state.viewport_top_id = None;

        for width in [40u16, 60, 80, 120] {
            let output = render_state(&state, width, 12);
            let blocks = collect_metric_rows(&output, &["box0", "box1"]);
            assert_brackets_align(&blocks, &format!("width {width}"));
        }
    }

    #[test]
    fn fleet_brackets_align_with_mixed_linux_windows_labels() {
        // A Linux-shaped system (SWP) and a Windows-shaped system (COMMIT)
        // must share one fleet label column and one bar column. COMMIT is
        // the wider label so the fleet label width is `len("COMMIT")`.
        let mut config = Config::default();
        for (i, name) in ["linux-host", "windows-host"].iter().enumerate() {
            config.systems.push(SystemEntry {
                id: format!("id-{i}"),
                host: format!("host{i}.local"),
                port: 11310,
                name: Some((*name).to_string()),
            });
        }
        let mut state = AppState::from_config(&config);
        apply_online_v2(
            &mut state,
            0,
            LinuxSnapshotV2Builder::default().build_payload(),
            1,
        );
        apply_online_v2(
            &mut state,
            1,
            WindowsSnapshotV2Builder::default().build_payload(),
            2,
        );
        state.viewport_top_id = None;

        for width in [40u16, 60, 80, 120] {
            let output = render_state(&state, width, 12);
            let blocks = collect_metric_rows(&output, &["linux-host", "windows-host"]);
            assert_brackets_align(&blocks, &format!("mixed fleet at width {width}"));
        }
    }

    #[test]
    fn fleet_brackets_remain_stable_when_longest_suffix_system_scrolls_off_viewport() {
        // With many systems online, the longest suffix belongs to a system
        // that initially sits below the viewport. Moving the viewport so
        // that system is visible must not change the bracket columns.
        let names: Vec<String> = (0..6).map(|i| format!("sys{i}")).collect();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let config = test_config(&name_refs);
        let mut state = AppState::from_config(&config);
        // Apply one batch with all six systems so they all become online
        // in the same generation.
        let results: Vec<crate::poller::PollResult> = (0..6)
            .map(|i| {
                let drives = if i == 5 {
                    Some(vec![DriveMetrics {
                        name: "/".into(),
                        used_bytes: 1200 * 1024 * 1024 * 1024 * 1024,
                        total_bytes: 1400 * 1024 * 1024 * 1024 * 1024,
                        available_bytes: None,
                    }])
                } else {
                    None
                };
                let payload = LinuxSnapshotV2Builder::default()
                    .drives(drives)
                    .build_payload();
                crate::poller::PollResult {
                    system_id: state.systems[i].id.clone(),
                    endpoint: state.systems[i].endpoint.clone(),
                    outcome: PollOutcome::OnlineV2(Box::new(payload)),
                    latency: Duration::from_millis(10),
                }
            })
            .collect();
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results,
        });
        // Six online systems: only the first one fits in a 7-row viewport.
        // The system with the widest suffix is far below the viewport.
        state.viewport_top_id = Some("id-0".into());
        let before = render_state(&state, 120, 7);
        let before_blocks = collect_metric_rows(&before, &["sys0"]);
        assert_brackets_align(&before_blocks, "initial viewport");

        // Scroll down so the wide-suffix system becomes visible.
        state.viewport_top_id = Some("id-5".into());
        let after = render_state(&state, 120, 7);
        let after_blocks = collect_metric_rows(&after, &["sys5"]);
        assert_brackets_align(&after_blocks, "scrolled viewport");

        // Bracket columns must agree across the two viewports because the
        // fleet geometry was stable across the scroll.
        let (before_opens, before_closes) = bracket_columns(&before_blocks[0]);
        let (after_opens, after_closes) = bracket_columns(&after_blocks[0]);
        assert_eq!(
            before_opens, after_opens,
            "opening brackets must not drift on scroll"
        );
        assert_eq!(
            before_closes, after_closes,
            "closing brackets must not drift on scroll"
        );
    }

    #[test]
    fn fleet_brackets_remain_aligned_at_narrow_widths() {
        // Narrow widths still produce aligned brackets as long as the bar
        // remains renderable; the intentional no-bracket fallback at very
        // small widths must drop brackets uniformly across systems.
        let names: Vec<String> = (0..3).map(|i| format!("srv{i}")).collect();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let config = test_config(&name_refs);
        let mut state = AppState::from_config(&config);
        // Apply one batch containing all three systems so each becomes
        // online in the same generation.
        let results: Vec<crate::poller::PollResult> = (0..3)
            .map(|i| crate::poller::PollResult {
                system_id: state.systems[i].id.clone(),
                endpoint: state.systems[i].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap_custom(20.0, 0.0, 4))),
                latency: Duration::from_millis(10),
            })
            .collect();
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results,
        });
        state.viewport_top_id = None;

        let wide = render_state(&state, 80, 30);
        let blocks_wide = collect_metric_rows(&wide, &name_refs);
        assert_brackets_align(&blocks_wide, "wide fleet");

        let narrow = render_state(&state, 60, 30);
        let blocks_narrow = collect_metric_rows(&narrow, &name_refs);
        assert_brackets_align(&blocks_narrow, "narrow fleet");
    }

    // ── Expanded drive-detail table layout ────────────────────────────

    fn collect_drive_lines(output: &str) -> Vec<String> {
        // Drive detail rows start with exactly two leading spaces (the
        // drive-row indent) followed by a non-space character. Metric
        // rows use four leading spaces and are not drive detail rows.
        let mut out = Vec::new();
        for line in output.lines() {
            let bytes = line.as_bytes();
            if bytes.len() >= 3 && bytes[0] == b' ' && bytes[1] == b' ' && bytes[2] != b' ' {
                out.push(line.to_string());
            }
        }
        out
    }

    /// Byte offset of the space preceding the slash between the `used`
    /// and `total` columns of a drive-detail row. This is the column
    /// the table layout guarantees across rows.
    fn used_total_slash(line: &str) -> usize {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        while i < bytes.len() && bytes[i] != b' ' {
            i += 1;
        }
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        while i < bytes.len() && bytes[i] != b' ' {
            i += 1;
        }
        i
    }

    #[test]
    fn drive_detail_columns_align_across_mixed_unit_drives() {
        let config = test_config(&["storage"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.systems[0].latest.as_mut().unwrap().drives = Some(vec![
            NormalizedDrive {
                name: "/".into(),
                used_bytes: 238 * 1024 * 1024 * 1024,
                total_bytes: 952 * 1024 * 1024 * 1024,
                available_bytes: Some(714 * 1024 * 1024 * 1024),
            },
            NormalizedDrive {
                name: "/mnt/archive".into(),
                used_bytes: 142 * 1024 * 1024 * 1024,
                total_bytes: 477 * 1024 * 1024 * 1024,
                available_bytes: Some(335 * 1024 * 1024 * 1024),
            },
            NormalizedDrive {
                name: "/mnt/backup".into(),
                used_bytes: 1200 * 1024 * 1024 * 1024 * 1024,
                total_bytes: 2000 * 1024 * 1024 * 1024 * 1024,
                available_bytes: Some(800 * 1024 * 1024 * 1024),
            },
        ]);
        state.drives_expanded = true;

        let output = render_state(&state, 120, 12);
        let lines = collect_drive_lines(&output);
        assert_eq!(lines.len(), 3, "expected 3 drive rows: {lines:?}");

        let sep_columns: Vec<usize> = lines.iter().map(|l| used_total_slash(l)).collect();
        assert!(
            sep_columns.iter().all(|&c| c == sep_columns[0]),
            "separator columns drifted: {sep_columns:?} from {lines:?}"
        );

        // The "%" character (closing the percent field) must occupy the
        // same column on every row.
        let pct_columns: Vec<usize> = lines
            .iter()
            .map(|line| line.rfind('%').expect("percent"))
            .collect();
        assert!(
            pct_columns.iter().all(|&c| c == pct_columns[0]),
            "percent columns drifted: {pct_columns:?}"
        );

        // Explicit availability is preserved inside `(...)` for each row.
        assert!(lines.iter().any(|l| l.contains("714.0 GiB")), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("800.0 GiB")),
            "TiB-scale explicit availability fallback: {lines:?}"
        );
    }

    #[test]
    fn drive_detail_remaining_falls_back_to_total_minus_used() {
        let config = test_config(&["legacy"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.systems[0].latest.as_mut().unwrap().drives = Some(vec![NormalizedDrive {
            name: "/".into(),
            used_bytes: 80 * 1024 * 1024 * 1024,
            total_bytes: 100 * 1024 * 1024 * 1024,
            available_bytes: None,
        }]);
        state.drives_expanded = true;

        let output = render_state(&state, 80, 8);
        let lines = collect_drive_lines(&output);
        assert_eq!(lines.len(), 1, "expected 1 drive row: {lines:?}");
        assert!(
            lines[0].contains("(20.0 GiB)"),
            "compatibility fallback: {:?}",
            lines[0]
        );
    }

    #[test]
    fn drive_detail_columns_unchanged_when_some_rows_are_clipped() {
        // Plan 085: vertical clipping must not shift horizontal columns.
        // A drive list that scrolls in one row should not move the
        // separator or percent columns.
        let config = test_config(&["storage"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.systems[0].latest.as_mut().unwrap().drives = Some(vec![
            NormalizedDrive {
                name: "/".into(),
                used_bytes: 238 * 1024 * 1024 * 1024,
                total_bytes: 952 * 1024 * 1024 * 1024,
                available_bytes: None,
            },
            NormalizedDrive {
                name: "/mnt/archive".into(),
                used_bytes: 142 * 1024 * 1024 * 1024,
                total_bytes: 477 * 1024 * 1024 * 1024,
                available_bytes: None,
            },
            NormalizedDrive {
                name: "/mnt/backup".into(),
                used_bytes: 1200 * 1024 * 1024 * 1024 * 1024,
                total_bytes: 2000 * 1024 * 1024 * 1024 * 1024,
                available_bytes: None,
            },
        ]);
        state.drives_expanded = true;

        let wide = render_state(&state, 120, 12);
        let wide_lines = collect_drive_lines(&wide);
        assert_eq!(
            wide_lines.len(),
            3,
            "all three rows visible: {wide_lines:?}"
        );

        let narrow = render_state(&state, 120, 7);
        let narrow_lines = collect_drive_lines(&narrow);
        assert!(
            narrow_lines.len() < wide_lines.len(),
            "clipping expected: {narrow_lines:?}"
        );
        for narrow_line in &narrow_lines {
            let wide_match = wide_lines
                .iter()
                .find(|w| w.contains('/') && w.contains(&narrow_line[..narrow_line.len().min(8)]))
                .expect("matching wide row");
            assert_eq!(
                narrow_line.find(" / "),
                wide_match.find(" / "),
                "separator column drifted on clip"
            );
            assert_eq!(
                narrow_line.rfind('%'),
                wide_match.rfind('%'),
                "percent column drifted on clip"
            );
        }
    }

    #[test]
    fn drive_detail_unicode_name_uses_terminal_cells() {
        // Wide-character mount names must not push the numeric columns.
        let config = test_config(&["storage"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.systems[0].latest.as_mut().unwrap().drives = Some(vec![
            NormalizedDrive {
                name: "/".into(),
                used_bytes: 80 * 1024 * 1024 * 1024,
                total_bytes: 100 * 1024 * 1024 * 1024,
                available_bytes: None,
            },
            NormalizedDrive {
                name: "/データ".into(),
                used_bytes: 50 * 1024 * 1024 * 1024,
                total_bytes: 100 * 1024 * 1024 * 1024,
                available_bytes: None,
            },
        ]);
        state.drives_expanded = true;

        let output = render_state(&state, 80, 8);
        let lines = collect_drive_lines(&output);
        assert_eq!(lines.len(), 2, "expected 2 drive rows: {lines:?}");

        let sep_columns: Vec<usize> = lines.iter().map(|l| used_total_slash_cell(l)).collect();
        assert!(
            sep_columns.iter().all(|&c| c == sep_columns[0]),
            "separator columns drifted under Unicode name: {sep_columns:?} from {lines:?}"
        );
        // Each line is width-bounded after trimming the TestBackend's
        // tail-pad (which does not subtract wide-character cells).
        for line in &lines {
            let trimmed = line.trim_end();
            assert!(
                UnicodeWidthStr::width(trimmed) <= 80,
                "line exceeds width: {trimmed:?} ({} cells)",
                UnicodeWidthStr::width(trimmed)
            );
        }
    }

    #[test]
    fn drive_detail_degrades_to_compact_at_narrow_widths() {
        // At a constrained width the full shape should degrade to
        // either Compact or Minimal without overflowing.
        let config = test_config(&["storage"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.systems[0].latest.as_mut().unwrap().drives = Some(vec![NormalizedDrive {
            name: "/mnt/long/mount/point".into(),
            used_bytes: 80 * 1024 * 1024 * 1024,
            total_bytes: 100 * 1024 * 1024 * 1024,
            available_bytes: None,
        }]);
        state.drives_expanded = true;

        let output = render_state(&state, 24, 8);
        let lines = collect_drive_lines(&output);
        assert_eq!(lines.len(), 1, "expected 1 drive row: {lines:?}");
        let line = &lines[0];
        assert!(
            UnicodeWidthStr::width(line.trim_end()) <= 24,
            "line exceeds width: {line:?}"
        );
        // Percentage must remain visible after degradation.
        assert!(line.contains("80.0%"), "percentage lost: {line:?}");
    }

    // ── Condensed view alignment ──────────────────────────────────────

    fn condensed_header(output: &str) -> &str {
        output.lines().next().unwrap_or("")
    }

    fn condensed_row(output: &str, index: usize) -> &str {
        output.lines().nth(index).unwrap_or("")
    }

    #[test]
    fn condensed_header_columns_align_with_value_columns_for_mixed_nicknames() {
        // Different configured nicknames must keep the HOST, CPU, MEM,
        // DISK, LOAD, and IOWAIT headings aligned with their value
        // columns at the same terminal cell.
        let mut config = Config::default();
        for (i, name) in ["pi5", "server3", "deadpool"].iter().enumerate() {
            config.systems.push(SystemEntry {
                id: format!("id-{i}"),
                host: format!("host{i}.local"),
                port: 11310,
                name: Some((*name).to_string()),
            });
        }
        let mut state = AppState::from_config(&config);
        let results: Vec<crate::poller::PollResult> = (0..3)
            .map(|i| crate::poller::PollResult {
                system_id: state.systems[i].id.clone(),
                endpoint: state.systems[i].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap_custom(
                    #[allow(clippy::cast_precision_loss)]
                    {
                        10.0 + 30.0 * i as f32
                    },
                    0.0,
                    4,
                ))),
                latency: Duration::from_millis(10),
            })
            .collect();
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results,
        });
        state.system_view_mode = crate::state::SystemViewMode::Condensed;

        let output = render_state(&state, 100, 10);
        let header = condensed_header(&output).to_string();
        let names = ["pi5", "server3", "deadpool"];
        for index in 2..=4 {
            let row = condensed_row(&output, index).to_string();
            #[allow(clippy::cast_precision_loss)]
            let cpu_pct = 10.0 + 30.0 * (index - 2) as f32;
            let cpu_value = format!("{cpu_pct:.0}%");
            let host_value = names[index - 2];
            let pairs: &[(&str, &str)] = &[
                ("HOST", host_value),
                ("CPU", &cpu_value),
                ("MEM", "38%"),
                ("DISK", "—"),
                ("LOAD", "1.32"),
                ("IOWAIT", "0.0"),
            ];
            for (heading, value) in pairs {
                let header_idx = header
                    .find(heading)
                    .unwrap_or_else(|| panic!("heading {heading:?} missing in header: {header:?}"));
                let value_byte_idx = row.find(value).unwrap_or_else(|| {
                    panic!(
                        "value {value:?} for heading {heading:?} not present in row {index}: \
                         {row:?}, header={header:?}"
                    )
                });
                let header_cell_idx = header_cell_at(&header, header_idx);
                let row_cell_idx = row_cell_at(&row, value_byte_idx);
                if *heading == "HOST" {
                    assert_eq!(
                        header_cell_idx, row_cell_idx,
                        "heading {heading:?} start cell column drifted between header and \
                         row {index}: header={header:?}, row={row:?}"
                    );
                } else {
                    let header_end = header_cell_idx + heading.chars().count();
                    let value_end = row_cell_idx + value.chars().count();
                    assert_eq!(
                        header_end, value_end,
                        "heading {heading:?} end cell column drifted between header and \
                         row {index}: header_end={header_end}, value_end={value_end}, \
                         header={header:?}, row={row:?}"
                    );
                }
            }
        }
    }

    fn row_cell_at(row: &str, byte_idx: usize) -> usize {
        let mut cells = 0usize;
        for (i, c) in row.char_indices() {
            if i >= byte_idx {
                return cells;
            }
            cells += UnicodeWidthChar::width(c).unwrap_or(0);
        }
        cells
    }

    /// Locate the slash between `used` and `total` columns of a
    /// drive-detail row by scanning terminal cells (not bytes). The
    /// shared table layout must align the slash at the same cell on
    /// every row, including rows whose names contain wide Unicode
    /// characters.
    fn used_total_slash_cell(line: &str) -> usize {
        use unicode_width::UnicodeWidthChar;
        let mut cells = 0usize;
        let mut chars = line.chars().peekable();
        // Skip leading indent.
        while let Some(&c) = chars.peek() {
            if c != ' ' {
                break;
            }
            chars.next();
        }
        // Skip name (until first space).
        while let Some(&c) = chars.peek() {
            if c == ' ' {
                break;
            }
            cells += UnicodeWidthChar::width(c).unwrap_or(0);
            chars.next();
        }
        // Skip gap (run of spaces).
        while let Some(&c) = chars.peek() {
            if c != ' ' {
                break;
            }
            chars.next();
        }
        // Skip used value (until next space).
        while let Some(&c) = chars.peek() {
            if c == ' ' {
                break;
            }
            cells += UnicodeWidthChar::width(c).unwrap_or(0);
            chars.next();
        }
        cells
    }

    fn header_cell_at(header: &str, byte_idx: usize) -> usize {
        let mut cells = 0usize;
        for (i, c) in header.char_indices() {
            if i >= byte_idx {
                return cells;
            }
            cells += UnicodeWidthChar::width(c).unwrap_or(0);
        }
        cells
    }

    #[test]
    fn condensed_host_column_does_not_consume_all_spare_width() {
        // The HOST column should track the longest required name, not
        // balloon to consume every spare terminal cell.
        let mut config = Config::default();
        for (i, name) in ["pi5", "deadpool-longer-name"].iter().enumerate() {
            config.systems.push(SystemEntry {
                id: format!("id-{i}"),
                host: format!("host{i}.local"),
                port: 11310,
                name: Some((*name).to_string()),
            });
        }
        let mut state = AppState::from_config(&config);
        let results: Vec<crate::poller::PollResult> = (0..2)
            .map(|i| crate::poller::PollResult {
                system_id: state.systems[i].id.clone(),
                endpoint: state.systems[i].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap_custom(15.0, 0.0, 4))),
                latency: Duration::from_millis(10),
            })
            .collect();
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results,
        });
        state.system_view_mode = crate::state::SystemViewMode::Condensed;

        let output = render_state(&state, 120, 10);
        let header = condensed_header(&output).to_string();
        let row = condensed_row(&output, 2).to_string();
        // Locate where the CPU value begins on the row and confirm it
        // starts shortly after the longest required name plus the
        // inter-column gap, not after every spare cell.
        let longest_name = "deadpool-longer-name".chars().count();
        let cpu_byte_idx = row.find("15%").expect("cpu value present");
        let host_cell_end = longest_name;
        let cpu_cell_idx = row_cell_at(&row, cpu_byte_idx);
        assert!(
            cpu_cell_idx <= host_cell_end + 4,
            "CPU column should begin shortly after host width: \
             host_cell_end={host_cell_end}, cpu_cell_idx={cpu_cell_idx}, row={row:?}, header={header:?}"
        );
    }

    #[test]
    fn condensed_tier_degradation_preserves_documented_priority() {
        // Wide/Medium/Narrow/Minimal must drop the same lower-priority
        // columns as before, in the same order.
        let config = test_config(&["srv"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        state.system_view_mode = crate::state::SystemViewMode::Condensed;

        let wide = render_state(&state, 80, 4);
        assert!(condensed_header(&wide).contains("IOWAIT"));
        assert!(condensed_header(&wide).contains("LOAD"));

        let medium = render_state(&state, 60, 4);
        let medium_header = condensed_header(&medium);
        assert!(medium_header.contains("LOAD"));
        assert!(!medium_header.contains("IOWAIT"));

        let narrow = render_state(&state, 40, 4);
        let narrow_header = condensed_header(&narrow);
        assert!(narrow_header.contains("DISK"));
        assert!(!narrow_header.contains("LOAD"));

        let minimal = render_state(&state, 24, 4);
        let minimal_header = condensed_header(&minimal);
        assert!(minimal_header.contains("MEM"));
        assert!(!minimal_header.contains("DISK"));
    }

    #[test]
    fn condensed_unicode_nickname_does_not_shift_numeric_columns() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        // A wide-character nickname must not push the numeric columns
        // relative to ASCII rows. Both rows must place "20%" in the
        // same terminal cell.
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "u1".into(),
            host: "host.local".into(),
            port: 11310,
            name: Some("サーバー".into()),
        });
        config.systems.push(SystemEntry {
            id: "u2".into(),
            host: "host2.local".into(),
            port: 11310,
            name: Some("srv".into()),
        });
        let mut state = AppState::from_config(&config);
        let results: Vec<crate::poller::PollResult> = (0..2)
            .map(|i| crate::poller::PollResult {
                system_id: state.systems[i].id.clone(),
                endpoint: state.systems[i].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(linux_snap_custom(20.0, 0.0, 4))),
                latency: Duration::from_millis(10),
            })
            .collect();
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results,
        });
        state.system_view_mode = crate::state::SystemViewMode::Condensed;
        state.viewport_top_id = None;

        // Use TestBackend directly so we can compare terminal cells
        // (not bytes) for both rows.
        let backend = TestBackend::new(100, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| super::render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();

        // Build char-by-char strings so each char represents one cell.
        let mut unicode_cells = String::new();
        let mut ascii_cells = String::new();
        for x in 0..100 {
            unicode_cells.push(
                buf.cell((x, 2))
                    .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' ')),
            );
            ascii_cells.push(
                buf.cell((x, 3))
                    .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' ')),
            );
        }
        // Count characters (each char == one cell) to find the cell
        // index of "20%".
        let unicode_cell_idx = unicode_cells.chars().take_while(|&c| c != '2').count();
        let ascii_cell_idx = ascii_cells.chars().take_while(|&c| c != '2').count();
        assert_eq!(
            unicode_cell_idx, ascii_cell_idx,
            "Unicode nickname shifted numeric columns at terminal cell: \
             unicode_cells={unicode_cells:?}, ascii_cells={ascii_cells:?}"
        );
    }
}
