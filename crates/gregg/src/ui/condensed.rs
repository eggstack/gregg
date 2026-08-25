//! The compact one-row-per-system fleet view.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use crate::normalized::aggregate_drives;
use crate::state::{Reachability, SystemState};

use super::text;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tier {
    Wide,
    Medium,
    Narrow,
    Minimal,
}

impl Tier {
    fn columns(self) -> &'static [Column] {
        match self {
            Tier::Wide => &[
                Column::Host,
                Column::Cpu,
                Column::Mem,
                Column::Disk,
                Column::Load,
                Column::Iowait,
            ],
            Tier::Medium => &[
                Column::Host,
                Column::Cpu,
                Column::Mem,
                Column::Disk,
                Column::Load,
            ],
            Tier::Narrow => &[Column::Host, Column::Cpu, Column::Mem, Column::Disk],
            Tier::Minimal => &[Column::Host, Column::Cpu, Column::Mem],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Column {
    Host,
    Cpu,
    Mem,
    Disk,
    Load,
    Iowait,
}

impl Column {
    fn heading(self) -> &'static str {
        match self {
            Column::Host => "HOST",
            Column::Cpu => "CPU",
            Column::Mem => "MEM",
            Column::Disk => "DISK",
            Column::Load => "LOAD",
            Column::Iowait => "IOWAIT",
        }
    }
}

fn tier(width: u16) -> Tier {
    match width {
        64..=u16::MAX => Tier::Wide,
        48..=63 => Tier::Medium,
        30..=47 => Tier::Narrow,
        _ => Tier::Minimal,
    }
}

/// Shared condensed-view column geometry. The same instance is used for
/// the header and every online/offline/pending row so headings and
/// values occupy identical terminal columns.
#[derive(Debug, Clone)]
pub(crate) struct CondensedTableLayout {
    tier: Tier,
    host_width: usize,
    cpu_width: usize,
    mem_width: usize,
    disk_width: usize,
    load_width: usize,
    iowait_width: usize,
    /// Display-cell width of every column plus the inter-column gap.
    /// Stored for diagnostic assertions.
    total_width: usize,
}

impl CondensedTableLayout {
    /// Display-cell width of one column. Returns 0 when the column is
    /// not part of the active tier.
    pub(crate) fn column_width(&self, column: Column) -> usize {
        match column {
            Column::Host => self.host_width,
            Column::Cpu => self.cpu_width,
            Column::Mem => self.mem_width,
            Column::Disk => self.disk_width,
            Column::Load => self.load_width,
            Column::Iowait => self.iowait_width,
        }
    }

    /// Active width tier chosen by the layout.
    pub(crate) fn tier(&self) -> Tier {
        self.tier
    }

    /// Total display-cell width of every column plus inter-column gaps.
    pub(crate) fn total_width(&self) -> usize {
        self.total_width
    }
}

const COLUMN_GAP_CELLS: usize = 2;

/// Pre-format every online system's value cells so the layout can pick
/// the widest value per column across the fleet.
#[derive(Debug, Clone)]
struct PreformattedValues {
    host: String,
    cpu: String,
    mem: String,
    disk: String,
    load: String,
    iowait: String,
}

fn preformat_online(system: &SystemState) -> PreformattedValues {
    let host = system
        .configured_name
        .as_deref()
        .unwrap_or(&system.endpoint.host)
        .to_string();
    let Some(snap) = system.latest.as_ref() else {
        return PreformattedValues {
            host,
            cpu: "—".into(),
            mem: "—".into(),
            disk: "—".into(),
            load: "—".into(),
            iowait: "—".into(),
        };
    };
    let cpu = format!("{:.0}%", snap.usage_pct.clamp(0.0, 100.0));
    let mem = format!("{:.0}%", snap.memory.usage_pct.clamp(0.0, 100.0));
    let disk = aggregate_drives(snap.drives.as_deref().unwrap_or_default()).map_or_else(
        || "—".to_string(),
        |aggregate| format!("{:.0}%", aggregate.usage_pct),
    );
    let load = snap
        .load
        .as_ref()
        .map_or_else(|| "—".to_string(), |load| format!("{:.2}", load.one));
    let iowait = snap
        .iowait_pct
        .filter(|_| snap.cpu_iowait_supported)
        .map_or_else(|| "—".to_string(), |value| format!("{value:.1}"));
    PreformattedValues {
        host,
        cpu,
        mem,
        disk,
        load,
        iowait,
    }
}

fn column_max(values: &[PreformattedValues], column: Column) -> usize {
    let from_values = values
        .iter()
        .map(|v| match column {
            Column::Host => cell_width(&v.host),
            Column::Cpu => cell_width(&v.cpu),
            Column::Mem => cell_width(&v.mem),
            Column::Disk => cell_width(&v.disk),
            Column::Load => cell_width(&v.load),
            Column::Iowait => cell_width(&v.iowait),
        })
        .max()
        .unwrap_or(0);
    from_values.max(cell_width(column.heading()))
}

fn cell_width(s: &str) -> usize {
    let mut w = 0usize;
    for ch in s.chars() {
        w += UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    w
}

fn pad_right(value: &str, width: usize) -> String {
    let used = cell_width(value);
    format!("{value}{}", " ".repeat(width.saturating_sub(used)))
}

fn pad_left(value: &str, width: usize) -> String {
    let used = cell_width(value);
    format!("{}{}", " ".repeat(width.saturating_sub(used)), value)
}

/// Compute the fleet-wide condensed table layout.
///
/// `systems` is the full configured fleet (online/offline/pending).
/// The active tier is chosen first by the terminal width, then
/// re-checked once the column widths are known: when the natural
/// widths would overflow the available terminal width, HOST is
/// truncated first, and finally the layout falls back to the next
/// narrower tier.
pub(crate) fn compute_condensed_table_layout(
    systems: &[SystemState],
    width: u16,
) -> CondensedTableLayout {
    let available = usize::from(width);
    let online_values: Vec<PreformattedValues> = systems
        .iter()
        .filter(|s| s.reachability == Reachability::Online)
        .map(preformat_online)
        .collect();
    // Plan 086: the HOST width budget must include every visible
    // system name (online/offline/pending) so status rows do not get
    // their device identity erased when the online fleet happens to
    // have shorter nicknames.
    let host_max_value = systems
        .iter()
        .map(|s| {
            s.configured_name
                .as_deref()
                .unwrap_or(&s.endpoint.host)
                .to_string()
        })
        .map(|name| cell_width(&name))
        .max()
        .unwrap_or(0);

    let mut current_tier = tier(width);
    loop {
        let mut widths = [
            Column::Host,
            Column::Cpu,
            Column::Mem,
            Column::Disk,
            Column::Load,
            Column::Iowait,
        ]
        .map(|c| column_max(&online_values, c));
        widths[0] = widths[0].max(host_max_value);
        let total = total_layout_width(current_tier, &widths);

        if total <= available {
            return build_layout(current_tier, widths, total);
        }

        // Try shrinking the HOST column first.
        let gap_count = current_tier.columns().len().saturating_sub(1);
        let numeric_width: usize = current_tier
            .columns()
            .iter()
            .filter(|c| **c != Column::Host)
            .map(|c| match c {
                Column::Cpu => widths[1],
                Column::Mem => widths[2],
                Column::Disk => widths[3],
                Column::Load => widths[4],
                Column::Iowait => widths[5],
                Column::Host => 0,
            })
            .sum();
        let host_budget = available
            .saturating_sub(numeric_width)
            .saturating_sub(gap_count * COLUMN_GAP_CELLS);

        if host_budget >= 4 {
            let mut adjusted = widths;
            adjusted[0] = host_budget.min(widths[0]);
            let total = total_layout_width(current_tier, &adjusted);
            if total <= available {
                return build_layout(current_tier, adjusted, total);
            }
        }

        // Fall back to the next narrower tier.
        current_tier = match current_tier {
            Tier::Wide => Tier::Medium,
            Tier::Medium => Tier::Narrow,
            Tier::Narrow => Tier::Minimal,
            Tier::Minimal => {
                // Last resort at the narrowest tier: force the HOST
                // column down to whatever budget remains (at least one
                // cell) so the returned layout does not exceed the
                // terminal when a long name drove the overflow.
                widths[0] = widths[0].min(host_budget.max(1));
                let total = total_layout_width(current_tier, &widths);
                return build_layout(current_tier, widths, total);
            }
        };
    }
}

fn total_layout_width(tier: Tier, widths: &[usize; 6]) -> usize {
    let columns = tier.columns();
    let mut total = 0usize;
    for (idx, column) in columns.iter().enumerate() {
        let w = match column {
            Column::Host => widths[0],
            Column::Cpu => widths[1],
            Column::Mem => widths[2],
            Column::Disk => widths[3],
            Column::Load => widths[4],
            Column::Iowait => widths[5],
        };
        if idx > 0 {
            total += COLUMN_GAP_CELLS;
        }
        total += w;
    }
    total
}

fn build_layout(tier: Tier, widths: [usize; 6], total: usize) -> CondensedTableLayout {
    CondensedTableLayout {
        tier,
        host_width: widths[0],
        cpu_width: widths[1],
        mem_width: widths[2],
        disk_width: widths[3],
        load_width: widths[4],
        iowait_width: widths[5],
        total_width: total,
    }
}

/// Render the condensed column header and separator.
pub(crate) fn render_header(f: &mut Frame, area: Rect, layout: &CondensedTableLayout) {
    let header = render_header_line(layout);
    f.render_widget(Line::from(header), Rect { height: 1, ..area });
    f.render_widget(
        Line::from("─".repeat(usize::from(area.width))),
        Rect {
            y: area.y.saturating_add(1),
            height: 1,
            ..area
        },
    );
}

fn render_header_line(layout: &CondensedTableLayout) -> String {
    let mut line = String::new();
    for (idx, column) in layout.tier.columns().iter().enumerate() {
        if idx > 0 {
            line.push_str(&" ".repeat(COLUMN_GAP_CELLS));
        }
        line.push_str(&pad_right(column.heading(), layout.column_width(*column)));
    }
    line
}

/// Render one condensed online, offline, or pending entry.
pub(crate) fn render_entry(
    f: &mut Frame,
    area: Rect,
    system: &SystemState,
    layout: &CondensedTableLayout,
    is_visually_selected: bool,
    is_logically_selected: bool,
    drive_rows_visible: usize,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let style = if is_visually_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    // Plan 087: drive-detail visibility is governed by logical
    // selection so an already-expanded list survives highlight
    // timeout. The renderer ignores the parameter if the entry is
    // not the logically selected system.
    let _ = is_logically_selected;
    let line = match system.reachability {
        Reachability::Online => render_online_row(system, layout),
        Reachability::Offline => status_line(system, layout, "offline"),
        Reachability::Pending => status_line(system, layout, "pending"),
    };
    f.render_widget(
        Line::from(Span::styled(line, style)),
        Rect { height: 1, ..area },
    );

    if system.reachability == Reachability::Online && drive_rows_visible > 0 {
        if let Some(drives) = system
            .latest
            .as_ref()
            .and_then(|snapshot| snapshot.drives.as_deref())
        {
            // Plan 085: compute the table layout from every eligible
            // drive so vertical clipping never shifts horizontal columns.
            let lines = text::render_drive_detail_lines(drives, area.width);
            for (offset, line) in lines.into_iter().take(drive_rows_visible).enumerate() {
                let y = area
                    .y
                    .saturating_add(1 + u16::try_from(offset).unwrap_or(u16::MAX));
                if y >= area.y.saturating_add(area.height) {
                    break;
                }
                f.render_widget(
                    Line::from(Span::raw(line)),
                    Rect {
                        y,
                        height: 1,
                        ..area
                    },
                );
            }
        }
    }
}

fn render_online_row(system: &SystemState, layout: &CondensedTableLayout) -> String {
    let preformatted = preformat_online(system);
    let mut line = String::new();
    for (idx, column) in layout.tier.columns().iter().enumerate() {
        if idx > 0 {
            line.push_str(&" ".repeat(COLUMN_GAP_CELLS));
        }
        let value = match column {
            Column::Host => &preformatted.host,
            Column::Cpu => &preformatted.cpu,
            Column::Mem => &preformatted.mem,
            Column::Disk => &preformatted.disk,
            Column::Load => &preformatted.load,
            Column::Iowait => &preformatted.iowait,
        };
        let width = layout.column_width(*column);
        let padded = match column {
            Column::Host => pad_right(&text::truncate_width(value, width), width),
            _ => pad_left(value, width),
        };
        line.push_str(&padded);
    }
    line
}

fn status_line(system: &SystemState, layout: &CondensedTableLayout, status: &str) -> String {
    let name = system
        .configured_name
        .as_deref()
        .unwrap_or(&system.endpoint.host);
    // Plan 086: status rows use the full rendered row width rather than
    // the online HOST numeric-table cell. The online table header
    // already drives the HOST column width, so this branch must not
    // erase device identity when the online fleet has shorter names.
    let status_suffix = format!("  {status}");
    let status_width = cell_width(&status_suffix);
    let row_width = layout.total_width;
    let name_budget = row_width.saturating_sub(status_width);
    let truncated = text::truncate_width(name, name_budget);
    format!("{truncated}{status_suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::Endpoint;
    use crate::normalized::NormalizedSnapshot;
    use std::time::Instant;
    use unicode_width::UnicodeWidthStr;

    fn system(name: &str) -> SystemState {
        let snapshot = NormalizedSnapshot::from_v1(
            &gregg_protocol::test_support::LinuxSnapshotBuilder::default()
                .usage_pct(12.4)
                .memory(8_000_000_000, 16_000_000_000)
                .load(0.32, 0.2, 0.1)
                .iowait_pct(18.2)
                .build(),
        );
        SystemState {
            id: "id".into(),
            endpoint: Endpoint::new("fallback".into(), 11310, None),
            configured_name: Some(name.into()),
            reachability: Reachability::Online,
            latest: Some(snapshot),
            last_success_at: Some(Instant::now()),
            last_attempt_at: Some(Instant::now()),
            latency: None,
            last_error: None,
        }
    }

    #[test]
    fn tiers_drop_low_priority_columns() {
        let systems = vec![system("srv")];
        assert_eq!(tier(64), Tier::Wide);
        assert_eq!(tier(48), Tier::Medium);
        assert_eq!(tier(30), Tier::Narrow);
        assert_eq!(tier(20), Tier::Minimal);

        assert!(compute_condensed_table_layout(&systems, 64)
            .tier
            .columns()
            .contains(&Column::Iowait));
        assert!(!compute_condensed_table_layout(&systems, 48)
            .tier
            .columns()
            .contains(&Column::Iowait));
        assert!(!compute_condensed_table_layout(&systems, 30)
            .tier
            .columns()
            .contains(&Column::Load));
        assert!(!compute_condensed_table_layout(&systems, 24)
            .tier
            .columns()
            .contains(&Column::Disk));
    }

    #[test]
    fn minimal_tier_fallback_never_exceeds_available_width() {
        // A long name at an extremely narrow width used to return a
        // Minimal-tier layout whose total exceeded the terminal.
        let systems = vec![system("an-extremely-long-system-name-that-cannot-fit")];
        let layout = compute_condensed_table_layout(&systems, 12);
        assert_eq!(layout.tier(), Tier::Minimal);
        assert!(layout.total_width <= 12, "got {}", layout.total_width);
        assert!(layout.host_width >= 1);
    }

    #[test]
    fn header_and_row_use_the_same_layout() {
        let systems = vec![system("pi"), system("deadpool")];
        let layout = compute_condensed_table_layout(&systems, 80);
        let header = render_header_line(&layout);
        let row = render_online_row(&system("pi"), &layout);
        // Header and rows must be the same total width.
        assert_eq!(cell_width(&header), layout.total_width);
        assert_eq!(cell_width(&row), layout.total_width);

        // Header and values must share each column's start/end cells.
        for column in layout.tier.columns() {
            let heading = column.heading();
            let header_idx = header.find(heading).expect("heading present");
            assert_eq!(
                header_idx,
                column_start_cell(&header, *column, &layout),
                "header start for {column:?}"
            );
        }
    }

    fn column_start_cell(line: &str, column: Column, layout: &CondensedTableLayout) -> usize {
        let mut cells = 0usize;
        let columns = layout.tier.columns();
        for (idx, target) in columns.iter().enumerate() {
            if *target == column {
                return cells;
            }
            // Skip current column width.
            cells += layout.column_width(*target);
            // Skip the gap that follows non-final columns.
            if idx + 1 < columns.len() {
                cells += COLUMN_GAP_CELLS;
            }
            let _ = line;
        }
        cells
    }

    #[test]
    fn online_line_uses_compact_values() {
        let systems = vec![system("サーバー")];
        let layout = compute_condensed_table_layout(&systems, 80);
        let line = render_online_row(&system("サーバー"), &layout);
        assert!(line.contains("12%"), "{line:?}");
        assert!(line.contains("50%"), "{line:?}");
        assert!(line.contains("0.32"), "{line:?}");
        assert!(line.contains("18.2"), "{line:?}");
    }

    #[test]
    fn status_line_does_not_show_stale_metrics() {
        let mut sys = system("offline-host");
        sys.reachability = Reachability::Offline;
        let layout = compute_condensed_table_layout(std::slice::from_ref(&sys), 80);
        let line = status_line(&sys, &layout, "offline");
        assert!(line.contains("offline"), "{line:?}");
        assert!(!line.contains('%'), "{line:?}");
    }

    fn offline_system(name: Option<&str>, host: &str) -> SystemState {
        let mut sys = system(name.unwrap_or("online-only"));
        sys.configured_name = name.map(str::to_string);
        sys.endpoint = Endpoint::new(host.into(), 11310, None);
        sys.reachability = Reachability::Offline;
        sys.latest = None;
        sys
    }

    #[test]
    fn offline_system_with_nickname_preserves_identity() {
        let sys = offline_system(Some("deadpool"), "192.168.182.146");
        let layout = compute_condensed_table_layout(std::slice::from_ref(&sys), 80);
        let line = status_line(&sys, &layout, "offline");
        assert!(
            line.contains("deadpool"),
            "nickname must remain visible: {line:?}"
        );
        assert!(line.contains("offline"), "{line:?}");
        assert!(!line.contains('%'), "{line:?}");
        assert!(
            UnicodeWidthStr::width(line.as_str()) <= 80,
            "line must fit width: {line:?}"
        );
    }

    #[test]
    fn offline_system_without_nickname_uses_endpoint_host() {
        let sys = offline_system(None, "192.168.182.146");
        let layout = compute_condensed_table_layout(std::slice::from_ref(&sys), 80);
        let line = status_line(&sys, &layout, "offline");
        assert!(
            line.contains("192.168.182.146"),
            "endpoint host must remain visible: {line:?}"
        );
        assert!(line.contains("offline"), "{line:?}");
        assert!(!line.contains('%'), "{line:?}");
    }

    #[test]
    fn offline_fleet_with_mixed_name_lengths_preserves_identity() {
        let short = offline_system(Some("a"), "host1.local");
        let long = offline_system(Some("deadpool"), "192.168.182.146");
        let layout = compute_condensed_table_layout(&[short.clone(), long.clone()], 80);
        let short_line = status_line(&short, &layout, "offline");
        let long_line = status_line(&long, &layout, "offline");
        assert!(
            short_line.contains('a'),
            "short name must remain visible: {short_line:?}"
        );
        assert!(
            long_line.contains("deadpool"),
            "long name must remain visible: {long_line:?}"
        );
        assert!(short_line.contains("offline"), "{short_line:?}");
        assert!(long_line.contains("offline"), "{long_line:?}");
        assert!(!short_line.contains('%'), "{short_line:?}");
        assert!(!long_line.contains('%'), "{long_line:?}");
    }

    #[test]
    fn mixed_online_offline_fleet_preserves_longer_offline_name() {
        // Online system has a short configured name; offline system has a
        // longer one. The offline nickname must not be erased by the
        // online table's HOST width.
        let online = system("pi");
        let offline = offline_system(Some("deadpool"), "192.168.182.146");
        let layout = compute_condensed_table_layout(&[online, offline.clone()], 80);
        let line = status_line(&offline, &layout, "offline");
        assert!(
            line.contains("deadpool"),
            "longer offline name must survive: {line:?}"
        );
        assert!(line.contains("offline"), "{line:?}");
    }

    #[test]
    fn pending_status_preserves_identity() {
        let mut sys = offline_system(Some("deadpool"), "192.168.182.146");
        sys.reachability = Reachability::Pending;
        let layout = compute_condensed_table_layout(std::slice::from_ref(&sys), 80);
        let line = status_line(&sys, &layout, "pending");
        assert!(
            line.contains("deadpool"),
            "pending nickname must remain visible: {line:?}"
        );
        assert!(line.contains("pending"), "{line:?}");
        assert!(!line.contains('%'), "{line:?}");
    }

    #[test]
    fn unicode_offline_nickname_preserves_identity() {
        let mut sys = offline_system(Some("サーバー"), "192.168.182.146");
        sys.reachability = Reachability::Offline;
        let layout = compute_condensed_table_layout(std::slice::from_ref(&sys), 80);
        let line = status_line(&sys, &layout, "offline");
        let width = UnicodeWidthStr::width(line.as_str());
        assert!(
            line.contains("サーバー"),
            "unicode nickname must remain visible: {line:?}"
        );
        assert!(line.contains("offline"), "{line:?}");
        assert!(width <= 80, "line must fit width: {line:?} ({width} cells)");
    }

    #[test]
    fn status_line_remains_width_bounded_at_narrow_widths() {
        let sys = offline_system(Some("deadpool"), "192.168.182.146");
        let layout = compute_condensed_table_layout(std::slice::from_ref(&sys), 24);
        let line = status_line(&sys, &layout, "offline");
        let width = UnicodeWidthStr::width(line.as_str());
        assert!(width <= 24, "line must fit width: {line:?} ({width} cells)");
        assert!(line.contains("offline"), "{line:?}");
    }
}
