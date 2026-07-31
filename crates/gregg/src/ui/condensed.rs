//! The compact one-row-per-system fleet view.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::normalized::aggregate_drives;
use crate::state::{Reachability, SystemState};

use super::text;

#[derive(Clone, Copy)]
enum Tier {
    Wide,
    Medium,
    Narrow,
    Minimal,
}

fn tier(width: u16) -> Tier {
    match width {
        64..=u16::MAX => Tier::Wide,
        48..=63 => Tier::Medium,
        30..=47 => Tier::Narrow,
        _ => Tier::Minimal,
    }
}

/// Render the condensed column header and separator.
pub fn render_header(f: &mut Frame, area: Rect) {
    let header = header_line(area.width);
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

/// Render one condensed online, offline, or pending entry.
pub fn render_entry(
    f: &mut Frame,
    area: Rect,
    system: &SystemState,
    is_selected: bool,
    drive_rows_visible: usize,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let style = if is_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    let line = match system.reachability {
        Reachability::Online => online_line(system, area.width),
        Reachability::Offline => status_line(system, area.width, "offline"),
        Reachability::Pending => status_line(system, area.width, "pending"),
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
            for (offset, drive) in drives
                .iter()
                .filter(|drive| drive.total_bytes > 0 && drive.used_bytes <= drive.total_bytes)
                .take(drive_rows_visible)
                .enumerate()
            {
                let y = area
                    .y
                    .saturating_add(1 + u16::try_from(offset).unwrap_or(u16::MAX));
                if y >= area.y.saturating_add(area.height) {
                    break;
                }
                f.render_widget(
                    Line::from(Span::raw(text::drive_detail_line(drive, area.width))),
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

fn header_line(width: u16) -> String {
    let columns = match tier(width) {
        Tier::Wide => "HOST CPU   MEM   DISK  LOAD  IOWAIT",
        Tier::Medium => "HOST CPU   MEM   DISK  LOAD",
        Tier::Narrow => "HOST CPU   MEM   DISK",
        Tier::Minimal => "HOST CPU   MEM",
    };
    text::truncate_width(columns, usize::from(width))
}

fn online_line(system: &SystemState, width: u16) -> String {
    let Some(snapshot) = system.latest.as_ref() else {
        return status_line(system, width, "—");
    };
    let host = system
        .configured_name
        .as_deref()
        .unwrap_or(&system.endpoint.host);
    let cpu = format!("{:.0}%", snapshot.usage_pct.clamp(0.0, 100.0));
    let mem = format!("{:.0}%", snapshot.memory.usage_pct.clamp(0.0, 100.0));
    let disk = aggregate_drives(snapshot.drives.as_deref().unwrap_or_default()).map_or_else(
        || "—".to_string(),
        |aggregate| format!("{:.0}%", aggregate.usage_pct),
    );
    let load = snapshot
        .load
        .as_ref()
        .map_or_else(|| "—".to_string(), |load| format!("{:.2}", load.one));
    let iowait = snapshot
        .iowait_pct
        .filter(|_| snapshot.cpu_iowait_supported)
        .map_or_else(|| "—".to_string(), |value| format!("{value:.1}"));

    let host_width = match tier(width) {
        Tier::Wide => usize::from(width).saturating_sub(29),
        Tier::Medium => usize::from(width).saturating_sub(22),
        Tier::Narrow => usize::from(width).saturating_sub(16),
        Tier::Minimal => usize::from(width).saturating_sub(10),
    };
    let host = pad_right(&text::truncate_width(host, host_width), host_width);
    match tier(width) {
        Tier::Wide => {
            format!("{host} {cpu:>4} {mem:>4} {disk:>5} {load:>5} {iowait:>6}")
        }
        Tier::Medium => format!("{host} {cpu:>4} {mem:>4} {disk:>5} {load:>5}"),
        Tier::Narrow => format!("{host} {cpu:>4} {mem:>4} {disk:>5}"),
        Tier::Minimal => format!("{host} {cpu:>4} {mem:>4}"),
    }
}

fn pad_right(value: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(value);
    format!("{value}{}", " ".repeat(width.saturating_sub(used)))
}

fn status_line(system: &SystemState, width: u16, status: &str) -> String {
    let name = system
        .configured_name
        .as_deref()
        .unwrap_or(&system.endpoint.host);
    let status_width = UnicodeWidthStr::width(status);
    let name_width = usize::from(width).saturating_sub(status_width + 2);
    format!("{}  {status}", text::truncate_width(name, name_width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::Endpoint;
    use crate::normalized::NormalizedSnapshot;
    use std::time::Instant;

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
        assert!(header_line(64).contains("IOWAIT"));
        assert!(!header_line(48).contains("IOWAIT"));
        assert!(!header_line(30).contains("LOAD"));
        assert!(!header_line(24).contains("DISK"));
    }

    #[test]
    fn online_line_is_width_bounded_and_uses_compact_values() {
        let line = online_line(&system("サーバー"), 64);
        assert_eq!(UnicodeWidthStr::width(line.as_str()), 64, "{line:?}");
        assert!(line.contains("12%"));
        assert!(line.contains("50%"));
        assert!(line.contains("0.32"));
        assert!(line.contains("18.2"));
    }

    #[test]
    fn status_line_does_not_show_stale_metrics() {
        let mut system = system("offline-host");
        system.reachability = Reachability::Offline;
        let line = status_line(&system, 30, "offline");
        assert!(line.contains("offline"));
        assert!(!line.contains('%'));
    }
}
