#![allow(dead_code)]

//! Normal-view system block rendering.
//!
//! The five-row online block is laid out in this fixed order:
//!
//! 1. Header line (priority-aware in `text::header_line`)
//! 2. CPU row
//! 3. MEM row
//! 4. SWP or COMMIT row (platform-determined)
//! 5. DISK aggregate row, optionally followed by per-drive detail rows
//!
//! Rows 2 through 5 share one geometry calculation so the opening and
//! closing brackets line up exactly. See `compute_metric_group_layout`
//! for the layout algorithm.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::normalized::{aggregate_drives, NormalizedSnapshot};
use crate::state::SystemState;

use super::bar;
use super::text;

/// Indent applied to every metric row in normal view.
const METRIC_ROW_INDENT: &str = "    ";

/// Render a normal-view online system block.
#[allow(clippy::too_many_lines)]
pub fn render_online(
    f: &mut Frame,
    area: Rect,
    system: &SystemState,
    is_selected: bool,
    drive_rows_visible: usize,
) {
    if area.height < 5 || area.width == 0 {
        return;
    }

    let Some(snap) = &system.latest else {
        render_waiting(f, area, system, is_selected);
        return;
    };

    let sel_style = if is_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    // Row 0: header line
    let header = text::header_line(system, area.width);
    let header_line = Line::from(Span::styled(header, sel_style));
    f.render_widget(header_line, Rect { height: 1, ..area });

    // Build the four metric rows and resolve the shared geometry once.
    let rows = build_metric_rows(snap);
    let layout = compute_metric_group_layout(&rows, area.width);

    let row_areas = [
        Rect {
            y: area.y.saturating_add(1),
            height: 1,
            ..area
        },
        Rect {
            y: area.y.saturating_add(2),
            height: 1,
            ..area
        },
        Rect {
            y: area.y.saturating_add(3),
            height: 1,
            ..area
        },
        Rect {
            y: area.y.saturating_add(4),
            height: 1,
            ..area
        },
    ];

    for (idx, row) in rows.iter().enumerate() {
        render_metric_row(f, row_areas[idx], row, &layout, idx);
    }

    render_drive_details(f, area, snap, drive_rows_visible);
}

fn render_waiting(f: &mut Frame, area: Rect, system: &SystemState, is_selected: bool) {
    let sel_style = if is_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    let header = text::header_line(system, area.width);
    f.render_widget(
        Line::from(Span::styled(header, sel_style)),
        Rect { height: 1, ..area },
    );
    f.render_widget(
        Line::from(Span::styled("waiting for data…", sel_style)),
        Rect {
            y: area.y.saturating_add(1),
            height: 1,
            ..area
        },
    );
}

/// One metric row inside the normal-view block.
#[derive(Debug, Clone)]
struct MetricRow {
    /// Label such as `CPU`, `MEM`, `SWP`, `COMMIT`, `DISK`.
    label: &'static str,
    /// Percentage; `None` means the metric is unavailable and the row
    /// should render the shared unavailable marker instead of fabricating
    /// a zero percentage.
    pct: Option<f32>,
    /// Optional detail printed after the percentage when width allows.
    detail: Option<String>,
}

impl MetricRow {
    /// Available percentage plus optional detail, as the line content
    /// will be built. For unavailable rows returns the shared
    /// unavailable marker.
    fn default_suffix(&self) -> String {
        match self.pct {
            None => "—".to_string(),
            Some(p) => {
                let pct = text::format_pct(p);
                match &self.detail {
                    None => pct,
                    Some(d) if d.is_empty() => pct,
                    Some(d) => format!("{pct} {d}"),
                }
            }
        }
    }

    /// Percentage-only suffix used when detail must be dropped to fit.
    fn percentage_only_suffix(&self) -> String {
        match self.pct {
            None => "—".to_string(),
            Some(p) => text::format_pct(p),
        }
    }
}

fn build_metric_rows(snap: &NormalizedSnapshot) -> [MetricRow; 4] {
    let cpu = MetricRow {
        label: "CPU",
        pct: Some(snap.usage_pct),
        detail: Some(format!("{} cores", snap.logical_cores)),
    };

    let mem = MetricRow {
        label: "MEM",
        pct: Some(snap.memory.usage_pct),
        detail: Some(format!(
            "{}/{}",
            text::format_bytes(snap.memory.used_bytes),
            text::format_bytes(snap.memory.total_bytes)
        )),
    };

    let third = match (&snap.swap, &snap.commit) {
        (Some(swap_info), _) => {
            let detail = if swap_info.total_bytes == 0 {
                None
            } else {
                Some(format!(
                    "{}/{}",
                    text::format_bytes(swap_info.used_bytes),
                    text::format_bytes(swap_info.total_bytes)
                ))
            };
            MetricRow {
                label: "SWP",
                pct: Some(swap_info.usage_pct),
                detail,
            }
        }
        (None, Some(commit_info)) => MetricRow {
            label: "COMMIT",
            pct: Some(commit_info.usage_pct),
            detail: Some(format!(
                "{}/{}",
                text::format_bytes(commit_info.used_bytes),
                text::format_bytes(commit_info.limit_bytes)
            )),
        },
        (None, None) => MetricRow {
            label: "SWP",
            pct: None,
            detail: None,
        },
    };

    let disk = match snap.drives.as_deref().and_then(aggregate_drives) {
        Some(aggregate) => {
            let used = text::format_bytes(aggregate.used_bytes);
            let avail = text::format_bytes(aggregate.available_bytes);
            MetricRow {
                label: "DISK",
                pct: Some(aggregate.usage_pct),
                detail: Some(format!("{used} / {avail}")),
            }
        }
        None => MetricRow {
            label: "DISK",
            pct: None,
            detail: None,
        },
    };

    [cpu, mem, third, disk]
}

/// One resolved metric layout shared by every row in a single system
/// block.
#[derive(Debug, Clone)]
struct MetricGroupLayout {
    /// Display-cell width of the longest label in the block (after
    /// padding has been applied).
    label_width: u16,
    /// Common bar width for all four rows.
    bar_width: u16,
    /// Final per-row suffix strings already truncated to fit
    /// `suffix_budget`.
    suffixes: [String; 4],
}

/// Width budget reserved for the suffix of each metric row.
///
/// When the four rows have differing natural widths the layout helper
/// drops optional details and then truncates so the largest suffix
/// still fits. The plan guarantees the percentage is preserved.
fn compute_metric_group_layout(rows: &[MetricRow; 4], width: u16) -> MetricGroupLayout {
    let label_width = rows
        .iter()
        .map(|r| u16::try_from(r.label.len()).unwrap_or(u16::MAX))
        .max()
        .unwrap_or(0);

    // Fixed structural prefix/suffix widths:
    //   prefix:  METRIC_ROW_INDENT + label + ' ['
    //   suffix:  '] ' + suffix_text
    let prefix_w = u16::try_from(METRIC_ROW_INDENT.len())
        .unwrap_or(u16::MAX)
        .saturating_add(label_width)
        .saturating_add(2); // " [" appended
    let after_bracket_w: u16 = 2; // "] "

    // Suffix budget is whatever remains after reserving the prefix and
    // the trailing "] " separator. When the suffix budget is zero we
    // cannot render any detail; the bar reclaims whatever space is left.
    let suffix_budget = width.saturating_sub(prefix_w + after_bracket_w);

    let suffixes = resolve_metric_suffixes(rows, usize::from(suffix_budget));

    let bar_width = width
        .saturating_sub(prefix_w + after_bracket_w)
        .saturating_sub(max_suffix_width(&suffixes));

    MetricGroupLayout {
        label_width,
        bar_width,
        suffixes,
    }
}

fn max_suffix_width(suffixes: &[String; 4]) -> u16 {
    let max = suffixes
        .iter()
        .map(|s| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(0);
    u16::try_from(max).unwrap_or(u16::MAX)
}

fn resolve_metric_suffixes(rows: &[MetricRow; 4], budget: usize) -> [String; 4] {
    if budget == 0 {
        // No room for a suffix at all. The renderer still emits "] "
        // for layout consistency; an empty suffix keeps brackets adjacent.
        return [String::new(), String::new(), String::new(), String::new()];
    }

    // First pass: full details.
    let mut suffixes = [
        rows[0].default_suffix(),
        rows[1].default_suffix(),
        rows[2].default_suffix(),
        rows[3].default_suffix(),
    ];
    if max_suffix_display(&suffixes) <= budget {
        return suffixes;
    }

    // Second pass: drop optional details. Some metrics are unavailable
    // (no percentage) or have no detail field; those already collapsed
    // to the percentage-only form on the first pass.
    suffixes = [
        rows[0].percentage_only_suffix(),
        rows[1].percentage_only_suffix(),
        rows[2].percentage_only_suffix(),
        rows[3].percentage_only_suffix(),
    ];
    if max_suffix_display(&suffixes) <= budget {
        return suffixes;
    }

    // Third pass: truncate every suffix so the longest one fits. The
    // helper guarantees the returned width is `<= budget`.
    suffixes.map(|s| bar::truncate_to_cells(&s, budget))
}

fn max_suffix_display(suffixes: &[String; 4]) -> usize {
    suffixes
        .iter()
        .map(|s| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(0)
}

/// Render a single metric row using the shared group layout.
fn render_metric_row(
    f: &mut Frame,
    area: Rect,
    row: &MetricRow,
    layout: &MetricGroupLayout,
    idx: usize,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let label_padded = format!(
        "{}{label:<width$}",
        METRIC_ROW_INDENT,
        label = row.label,
        width = usize::from(layout.label_width)
    );
    let bar = make_bar_string(row.pct, layout.bar_width);
    let suffix_text = layout.suffixes.get(idx).map_or("", |s| s.as_str());

    let line = if layout.bar_width == 0 {
        // No bar budget at all — print label and suffix without brackets.
        if suffix_text.is_empty() {
            label_padded
        } else {
            format!("{label_padded}  {suffix_text}")
        }
    } else {
        format!("{label_padded} [{bar}] {suffix_text}")
    };

    bar::render_text_line(f, area, &line);
}

fn make_bar_string(pct: Option<f32>, bar_width: u16) -> String {
    match pct {
        Some(p) => {
            let clamped = p.clamp(0.0, 100.0);
            let width_f = f32::from(bar_width);
            // `clamped` is in [0, 100] and `width_f` fits in u16, so the
            // scaled value is non-negative and bounded by `width_f`. We
            // explicitly clamp before converting to avoid lossy-cast
            // lints (Rust 1.75 lacks `TryFrom<f32>` for integer targets).
            let scaled = (clamped * width_f) / 100.0;
            let clamped_scaled = if scaled.is_finite() && scaled >= 0.0 {
                scaled.min(f32::from(u16::MAX))
            } else {
                0.0
            };
            // The value is pre-clamped to [0, u16::MAX]; Rust 1.75 has no
            // TryFrom<f32> for this conversion.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let filled_u32 = clamped_scaled as u32;
            let filled = usize::try_from(filled_u32.min(u32::from(bar_width))).unwrap_or(0);
            let empty = (usize::from(bar_width)).saturating_sub(filled);
            format!("{}{}", "|".repeat(filled), " ".repeat(empty))
        }
        None => " ".repeat(usize::from(bar_width)),
    }
}

fn render_drive_details(
    f: &mut Frame,
    area: Rect,
    snap: &NormalizedSnapshot,
    drive_rows_visible: usize,
) {
    if drive_rows_visible == 0 {
        return;
    }
    let Some(drives) = snap.drives.as_deref() else {
        return;
    };
    for (offset, drive) in drives
        .iter()
        .filter(|drive| drive.total_bytes > 0 && drive.used_bytes <= drive.total_bytes)
        .take(drive_rows_visible)
        .enumerate()
    {
        let row = area
            .y
            .saturating_add(5 + u16::try_from(offset).unwrap_or(u16::MAX));
        if row >= area.y.saturating_add(area.height) {
            break;
        }
        let line = text::drive_detail_line(drive, area.width);
        bar::render_text_line(
            f,
            Rect {
                y: row,
                height: 1,
                ..area
            },
            &line,
        );
    }
}

/// Render a 1-row offline system line.
pub fn render_offline(f: &mut Frame, area: Rect, system: &SystemState, is_selected: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let style = if is_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    let status_text = match system.reachability {
        crate::state::Reachability::Offline => "offline",
        crate::state::Reachability::Pending => "pending",
        crate::state::Reachability::Online => "online",
    };

    // Named systems show `name@host:port`; unnamed systems render only
    // `host:port` so we never duplicate the host as both the synthetic
    // name and the address suffix.
    let prefix = match system.configured_name.as_deref() {
        Some(name) => format!("{name}@{}", system.endpoint.display_address()),
        None => system.endpoint.display_address(),
    };

    let prefix_with_status = format!("{prefix} {status_text} ");
    let total_width = area.width as usize;
    let used = UnicodeWidthStr::width(prefix_with_status.as_str());
    let dot_count = total_width.saturating_sub(used);
    let line_text = format!("{prefix_with_status}{}", ".".repeat(dot_count));

    let line = Line::from(Span::styled(line_text, style));
    f.render_widget(line, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &'static str, pct: f32, detail: Option<&str>) -> MetricRow {
        MetricRow {
            label,
            pct: Some(pct),
            detail: detail.map(str::to_string),
        }
    }

    #[test]
    fn label_width_uses_widest_actual_label() {
        let rows = [
            row("CPU", 1.0, None),
            row("MEM", 1.0, None),
            row("COMMIT", 1.0, None),
            row("DISK", 1.0, None),
        ];
        let layout = compute_metric_group_layout(&rows, 80);
        assert_eq!(layout.label_width, u16::try_from("COMMIT".len()).unwrap());
    }

    #[test]
    fn closing_brackets_align_when_details_fit() {
        let rows = [
            row("CPU", 25.2, Some("8 cores")),
            row("MEM", 37.8, Some("5.9 GiB / 15.6 GiB")),
            row("SWP", 0.0, Some("0 B / 4.0 GiB")),
            row("DISK", 60.4, Some("283.8 GiB / 167.1 GiB")),
        ];
        let layout = compute_metric_group_layout(&rows, 80);
        // The closing `]` must fall in the same terminal column for
        // every row because `bar_width` is shared.
        for (idx, row) in rows.iter().enumerate() {
            let line = build_row_line(row, idx, &layout);
            assert_eq!(
                line.rfind(']').unwrap(),
                4 /* indent */ + usize::from(layout.label_width) + 2 + usize::from(layout.bar_width),
                "row {:?} closing bracket drifted: {line:?}",
                row.label,
            );
        }
        // All brackets and suffixes end at the same absolute column.
        let closes: Vec<_> = [("CPU", 0usize), ("MEM", 1), ("SWP", 2), ("DISK", 3)]
            .iter()
            .map(|(label, idx)| {
                let line = build_row_line(&row(label, 0.0, None), *idx, &layout);
                line.rfind(']').unwrap()
            })
            .collect();
        assert!(
            closes.iter().all(|&c| c == closes[0]),
            "closing brackets must align across rows, got {closes:?}"
        );
    }

    fn build_row_line(row: &MetricRow, idx: usize, layout: &MetricGroupLayout) -> String {
        let label_padded = format!(
            "{}{label:<width$}",
            METRIC_ROW_INDENT,
            label = row.label,
            width = usize::from(layout.label_width)
        );
        let suffix = layout.suffixes.get(idx).map_or("", |s| s.as_str());
        if layout.bar_width == 0 {
            if suffix.is_empty() {
                label_padded
            } else {
                format!("{label_padded}  {suffix}")
            }
        } else {
            format!(
                "{label_padded} [{bar}] {suffix}",
                bar = make_bar_string(row.pct, layout.bar_width)
            )
        }
    }

    #[test]
    fn detail_dropped_when_budget_too_tight() {
        let rows = [
            row("CPU", 25.2, Some("8 cores")),
            row("MEM", 37.8, Some("5.9 GiB / 15.6 GiB")),
            row("SWP", 0.0, None),
            row("DISK", 60.4, Some("283.8 GiB / 167.1 GiB")),
        ];
        let layout = compute_metric_group_layout(&rows, 30);
        let max = suffixes_max_width(&layout.suffixes);
        for suffix in &layout.suffixes {
            assert!(
                UnicodeWidthStr::width(suffix.as_str()) <= max,
                "suffix must fit the resolved max width"
            );
        }
        for suffix in &layout.suffixes {
            assert!(suffix.contains('%'), "percentage must survive: {suffix:?}");
        }
    }

    #[test]
    fn truncation_respects_budget_for_display_cells() {
        let budget = 12;
        let suffix = bar::truncate_to_cells("283.8 GiB / 167.1 GiB", budget);
        assert!(UnicodeWidthStr::width(suffix.as_str()) <= budget);
    }

    #[test]
    fn truncation_at_zero_returns_empty() {
        assert_eq!(bar::truncate_to_cells("anything", 0), "");
    }

    #[test]
    fn unavailable_row_uses_em_dash_not_fabricated_zero() {
        let rows = [
            MetricRow {
                label: "CPU",
                pct: Some(50.0),
                detail: None,
            },
            MetricRow {
                label: "MEM",
                pct: Some(50.0),
                detail: None,
            },
            MetricRow {
                label: "SWP",
                pct: Some(50.0),
                detail: None,
            },
            MetricRow {
                label: "DISK",
                pct: None,
                detail: None,
            },
        ];
        let layout = compute_metric_group_layout(&rows, 80);
        let disk = &layout.suffixes[3];
        assert!(
            disk.contains('—'),
            "unavailable disk should use em-dash: {disk:?}"
        );
        assert!(
            !disk.contains("0.0%"),
            "unavailable disk must not fabricate 0.0%: {disk:?}"
        );
    }

    fn suffixes_max_width(suffixes: &[String; 4]) -> usize {
        suffixes
            .iter()
            .map(|s| UnicodeWidthStr::width(s.as_str()))
            .max()
            .unwrap_or(0)
    }
}
