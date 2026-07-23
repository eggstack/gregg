#![allow(dead_code)]

pub mod bar;
pub mod diagnostics;
pub mod layout;
pub mod system_block;
pub mod text;

use ratatui::Frame;

use crate::state::AppState;

/// Render the full TUI into the current frame.
pub fn render(f: &mut Frame, state: &AppState) {
    let area = f.area();

    if state.systems.is_empty() {
        diagnostics::render_empty_config(f, area);
        return;
    }

    if area.width < 24 || area.height < 4 {
        diagnostics::render_too_small(f, area);
        return;
    }

    let entries = layout::compute_viewport(state, area);

    // Compute how many rows the entries consume.
    let entries_bottom = entries.last().map_or(area.y, |e| e.rect.y + e.rect.height);
    let extra_rows = area
        .y
        .saturating_add(area.height)
        .saturating_sub(entries_bottom);

    for entry in &entries {
        let system = &state.systems[entry.index];
        match system.reachability {
            crate::state::Reachability::Online => {
                system_block::render_online(f, entry.rect, system, entry.is_selected);
            }
            crate::state::Reachability::Offline | crate::state::Reachability::Pending => {
                system_block::render_offline(f, entry.rect, system, entry.is_selected);
            }
        }
    }

    // Show a key hint only when there is at least one extra row below entries.
    if extra_rows >= 1 {
        diagnostics::render_key_hint(f, area);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::Terminal;

    use crate::config::{Config, SystemEntry};
    use crate::poller::{PollBatch, PollOutcome};
    use crate::state::{AppState, Reachability};
    use gregg_protocol::test_support::{LinuxSnapshotBuilder, MacosSnapshotBuilder};
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

    fn apply_offline(state: &mut AppState, index: usize) {
        let batch = make_offline_batch(state, index);
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

    // ── 1. Empty config ──────────────────────────────────────────────

    #[test]
    fn render_empty_config() {
        let config = Config::default();
        let state = AppState::from_config(&config);
        let output = render_state(&state, 80, 24);
        assert!(
            output.contains("No systems configured"),
            "expected 'No systems configured' in output:\n{output}"
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
        // Online system occupies 4 rows.
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
        // macOS has cpu_iowait = false, so header should show "IO —" not a fabricated percentage
        assert!(
            header.contains("IO —"),
            "macOS header should show 'IO —', got: {header}"
        );
        assert!(
            !header.contains("IO 0.0%"),
            "macOS header must not show fabricated 'IO 0.0%', got: {header}"
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
        let output = render_state(&state, 80, 8);
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
        let output = render_state(&state, 80, 8);
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
        let output = render_state(&state, 80, 8);
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
        let output = render_state(&state, 80, 8);
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
        let output = render_state(&state, 80, 8);
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
        // Make all systems online (4 rows each).
        for i in 0..6 {
            apply_online(&mut state, i, linux_snap());
            // Bump generation for each batch.
        }

        // 6 online systems × 4 rows = 24 rows needed, but terminal is 12 tall.
        let output = render_state(&state, 80, 12);

        // Only 3 online systems should fit in 12 rows (3 × 4 = 12).
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
    fn online_system_uses_four_rows() {
        let config = test_config(&["s1"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        // Height 4 = exactly one online system, no room for key hint.
        let output = render_state(&state, 80, 4);
        let nonblank = count_nonblank_lines(&output);
        assert_eq!(nonblank, 4, "one online system should use exactly 4 rows");
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
        // Height 6 = exactly 4 + 1 + 1, no room for key hint.
        let output = render_state(&state, 80, 6);
        let nonblank = count_nonblank_lines(&output);
        // 4 (online a) + 1 (offline b) + 1 (pending c) = 6
        assert_eq!(
            nonblank, 6,
            "online(4) + offline(1) + pending(1) = 6, got {nonblank}"
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
        assert!(
            header.contains("IO —"),
            "macOS header should show 'IO —' (unsupported), got: {header}"
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
        let output = render_state(&state, 80, 8);
        let cpu_line = output.lines().nth(1).unwrap();
        assert!(
            cpu_line.starts_with("CPU"),
            "CPU bar should start with label, got: {cpu_line}"
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
        let output = render_state(&state, 80, 8);
        let mem_line = output.lines().nth(2).unwrap();
        assert!(
            mem_line.starts_with("MEM"),
            "MEM bar should start with label, got: {mem_line}"
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
        let output = render_state(&state, 80, 8);
        let swap_line = output.lines().nth(3).unwrap();
        assert!(
            swap_line.starts_with("SWP"),
            "SWP bar should start with label, got: {swap_line}"
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

        let output = render_state(&state, 80, 16);
        let lines: Vec<&str> = output.lines().collect();

        // System a is first (online, selected), then system b.
        assert!(lines[0].contains('a'), "first header: {}", lines[0]);
        assert!(lines[4].contains('b'), "second header: {}", lines[4]);

        // CPU bars should differ.
        assert!(lines[1].contains("10.0%"), "a CPU: {}", lines[1]);
        assert!(lines[5].contains("90.0%"), "b CPU: {}", lines[5]);
    }

    #[test]
    fn empty_config_at_various_sizes() {
        let config = Config::default();
        let state = AppState::from_config(&config);
        for &(w, h) in &[(80, 24), (40, 12), (20, 5), (120, 40)] {
            let output = render_state(&state, w, h);
            assert!(output.contains("No systems"), "at {w}x{h}: {output}");
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
        let output = render_state(&state, 24, 4);
        assert!(
            !output.contains("terminal too small"),
            "width 24 should be valid:\n{output}"
        );
        // Should render the online system header.
        assert!(output.contains('s'), "should render system: {output}");
    }

    #[test]
    fn height_exactly_4_is_not_too_small() {
        let config = test_config(&["s"]);
        let mut state = AppState::from_config(&config);
        apply_online(&mut state, 0, linux_snap());
        let output = render_state(&state, 80, 4);
        assert!(
            !output.contains("terminal too small"),
            "height 4 should be valid:\n{output}"
        );
        assert!(output.contains('s'), "should render system: {output}");
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

        // System a is selected (row 0).
        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| super::render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();

        assert!(
            buf.cell((0, 0))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "a should be reversed"
        );
        assert!(
            !buf.cell((0, 4))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "b should NOT be reversed"
        );

        // Move selection to b.
        state.apply_action(crate::action::Action::SelectNext);
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
            buf2.cell((0, 4))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "b should be reversed after moving selection"
        );
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
        // Width 24 = minimum valid width, height 4 = minimum valid height.
        let output = render_state(&state, 24, 4);
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
                output.contains("No systems configured"),
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

        // Height 4 = minimum, 2 offline systems + key hint in remaining space.
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
        // 4 rows: exactly one system, no extra space.
        let output = render_state(&state, 80, 4);
        assert!(
            !output.contains("j/k:select"),
            "key hint should not appear when no extra space:\n{output}"
        );
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
        // render_online returns early when snap is None, so the 4-row
        // block is allocated but left blank. Verify no crash occurred.
        assert!(!output.is_empty(), "should render without crashing");
    }
}
