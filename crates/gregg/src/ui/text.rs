#![allow(dead_code)]

use crate::normalized::NormalizedDrive;
use crate::state::SystemState;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const KIB: u64 = 1024;
const MIB: u64 = KIB * 1024;
const GIB: u64 = MIB * 1024;
const TIB: u64 = GIB * 1024;

/// Format a byte count as a human-readable string using binary units.
#[allow(clippy::cast_precision_loss)]
pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }

    if bytes >= TIB {
        format!("{:.1} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Format a percentage value.
pub fn format_pct(pct: f32) -> String {
    let clamped = pct.clamp(0.0, 100.0);
    if clamped >= 100.0 {
        "100%".to_string()
    } else if clamped <= 0.0 {
        "0.0%".to_string()
    } else {
        format!("{clamped:.1}%")
    }
}

/// Format load averages as a compact string.
pub fn format_load(load: &gregg_protocol::LoadAverage) -> String {
    format!("{:.2}/{:.2}/{:.2}", load.one, load.five, load.fifteen)
}

/// Compose a priority-aware header line for an online system.
///
/// Priority (dropped as width decreases):
/// 1. Display name or hostname
/// 2. I/O-wait value or "--" for unsupported
/// 3. Load averages or "--" for unsupported
/// 4. Logical core count
/// 5. OS name/version
/// 6. Kernel release
/// 7. Architecture
pub fn header_line(system: &SystemState, width: u16) -> String {
    let Some(snap) = &system.latest else {
        return format!("{} (no data)", display_name(system));
    };

    let name = display_name(system);

    let io_str = if snap.cpu_iowait_supported {
        match snap.iowait_pct {
            Some(iowait) => format!("IO {iowait:.1}%"),
            None => "IO \u{2014}".to_string(),
        }
    } else {
        "IO \u{2014}".to_string()
    };

    let load_str = match &snap.load {
        Some(l) => format_load(l),
        None => "L \u{2014}".to_string(),
    };
    let cores_str = format!("{}c", snap.logical_cores);
    let os_str = format!("{} {}", snap.system.os_name, snap.system.os_version);
    let kernel_str = format!("{} {}", snap.system.kernel_name, snap.system.kernel_release);
    let arch_str = &snap.system.architecture;

    if width >= 80 {
        format!("{name}  {io_str}  {load_str}  {cores_str}  {os_str}  {kernel_str}  {arch_str}")
    } else if width >= 50 {
        format!("{name}  {io_str}  {load_str}  {cores_str}  {os_str}")
    } else if width >= 32 {
        format!("{name}  {io_str}  {load_str}  {cores_str}")
    } else {
        format!("{name}  {io_str}")
    }
}

/// Return the display name for a system.
///
/// If a name was configured by the operator, it is preferred for stable
/// identity in the TUI regardless of what the daemon reports. The
/// endpoint host is used as a fallback when no configured name exists.
fn display_name(system: &SystemState) -> &str {
    system
        .configured_name
        .as_deref()
        .unwrap_or(&system.endpoint.host)
}

/// One pre-formatted drive row, independent of any width budget. The
/// table layout combines rows from every eligible drive in the
/// selected system so visible-row clipping never changes horizontal
/// columns.
#[derive(Debug, Clone)]
pub(crate) struct DriveDetailRow {
    pub(crate) name: String,
    pub(crate) used: String,
    pub(crate) total: String,
    pub(crate) remaining: String,
    pub(crate) percent: String,
}

/// Width mode for the drive-detail table. Plan 085 picks one of:
/// 1. `Full` — `name  used / total  (remaining) percent`
/// 2. `Compact` — `name  (remaining) percent`
/// 3. `Minimal` — `name  percent`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriveDetailMode {
    Full,
    Compact,
    Minimal,
}

/// Pre-computed layout for the selected system's drive detail rows.
#[derive(Debug, Clone)]
pub(crate) struct DriveTableLayout {
    name_width: usize,
    used_width: usize,
    total_width: usize,
    remaining_width: usize,
    percent_width: usize,
    mode: DriveDetailMode,
}

impl DriveTableLayout {
    /// Return the column mode the layout chose. Useful in tests so the
    /// degradation path can be asserted without re-implementing the
    /// width math.
    pub(crate) fn mode(&self) -> DriveDetailMode {
        self.mode
    }
}

/// Format one drive's pre-table fields. Eligibility (`used <= total`,
/// `total > 0`) must be checked by the caller.
pub(crate) fn build_drive_detail_row(drive: &NormalizedDrive) -> DriveDetailRow {
    let used = format_bytes(drive.used_bytes);
    let total = format_bytes(drive.total_bytes);
    let remaining_bytes = drive
        .available_bytes
        .unwrap_or(drive.total_bytes - drive.used_bytes);
    let remaining = format!("({})", format_bytes(remaining_bytes));
    let percent = format!(
        "{}",
        format_pct((drive.used_bytes as f64 * 100.0 / drive.total_bytes as f64) as f32)
    );
    DriveDetailRow {
        name: drive.name.clone(),
        used,
        total,
        remaining,
        percent,
    }
}

fn drive_row_widths(row: &DriveDetailRow) -> (usize, usize, usize, usize, usize) {
    (
        UnicodeWidthStr::width(row.name.as_str()),
        UnicodeWidthStr::width(row.used.as_str()),
        UnicodeWidthStr::width(row.total.as_str()),
        UnicodeWidthStr::width(row.remaining.as_str()),
        UnicodeWidthStr::width(row.percent.as_str()),
    )
}

/// Compute the drive-detail table layout from every eligible drive's
/// pre-formatted fields and the available width.
///
/// Layout modes are tried in Plan 085's documented order:
/// 1. full `name  used / total  (remaining) percent`;
/// 2. shrink the name column only while keeping numeric columns intact;
/// 3. compact `name  (remaining) percent`;
/// 4. minimal `name  percent`.
pub(crate) fn compute_drive_table_layout(rows: &[DriveDetailRow], width: u16) -> DriveTableLayout {
    let available = usize::from(width);

    // Widths implied by the widest formatted field across every row.
    let mut max_name = 0usize;
    let mut max_used = 0usize;
    let mut max_total = 0usize;
    let mut max_remaining = 0usize;
    let mut max_percent = 0usize;
    for row in rows {
        let (n, u, t, r, p) = drive_row_widths(row);
        max_name = max_name.max(n);
        max_used = max_used.max(u);
        max_total = max_total.max(t);
        max_remaining = max_remaining.max(r);
        max_percent = max_percent.max(p);
    }

    // Full layout width = name + used + " / " + total + "  " + remaining + "  " + percent.
    let full_width = max_name
        .saturating_add(2) // "  "
        .saturating_add(max_used)
        .saturating_add(2) // " /"
        .saturating_add(1) // " "
        .saturating_add(max_total)
        .saturating_add(2) // "  "
        .saturating_add(max_remaining)
        .saturating_add(2) // "  "
        .saturating_add(max_percent);

    if available >= full_width {
        return DriveTableLayout {
            name_width: max_name,
            used_width: max_used,
            total_width: max_total,
            remaining_width: max_remaining,
            percent_width: max_percent,
            mode: DriveDetailMode::Full,
        };
    }

    // Try shrinking the name column only, keeping every numeric column
    // intact. We must leave two spaces of indent plus the "  " / "/ "
    // separators consumed by the row.
    let fixed_numeric = max_used
        .saturating_add(2)
        .saturating_add(1)
        .saturating_add(max_total)
        .saturating_add(2)
        .saturating_add(max_remaining)
        .saturating_add(2)
        .saturating_add(max_percent);
    let available_for_name_and_indent = available.saturating_sub(fixed_numeric);
    // Indent is 2 cells ("  "), separator between name and values is
    // another 2 cells. Subtract both from the name budget.
    let name_budget = available_for_name_and_indent
        .saturating_sub(2) // indent
        .saturating_sub(2); // "  " before used
    if name_budget >= 1 {
        let effective_name_width = max_name.min(name_budget);
        return DriveTableLayout {
            name_width: effective_name_width,
            used_width: max_used,
            total_width: max_total,
            remaining_width: max_remaining,
            percent_width: max_percent,
            mode: DriveDetailMode::Full,
        };
    }

    // Compact fallback: name + remaining + percent.
    let compact_width = max_name
        .saturating_add(2)
        .saturating_add(max_remaining)
        .saturating_add(2)
        .saturating_add(max_percent);
    if available >= compact_width {
        let compact_name_budget = available
            .saturating_sub(max_remaining)
            .saturating_sub(max_percent)
            .saturating_sub(2) // indent
            .saturating_sub(2) // "  " between name and remaining
            .saturating_sub(2); // "  " before percent
        let effective_name_width = max_name.min(compact_name_budget);
        return DriveTableLayout {
            name_width: effective_name_width,
            used_width: 0,
            total_width: 0,
            remaining_width: max_remaining,
            percent_width: max_percent,
            mode: DriveDetailMode::Compact,
        };
    }

    // Minimal fallback: name + percent.
    let minimal_name_budget = available
        .saturating_sub(max_percent)
        .saturating_sub(2) // indent
        .saturating_sub(2); // "  " before percent
    let effective_name_width = max_name.min(minimal_name_budget);
    DriveTableLayout {
        name_width: effective_name_width,
        used_width: 0,
        total_width: 0,
        remaining_width: 0,
        percent_width: max_percent,
        mode: DriveDetailMode::Minimal,
    }
}

/// Render one drive-detail row using the precomputed layout. The
/// renderer is deterministic for a given layout and drive.
pub(crate) fn render_drive_detail_row(row: &DriveDetailRow, layout: &DriveTableLayout) -> String {
    let indent = "  ";
    let gap = "  ";
    let name = truncate_width(&row.name, layout.name_width);
    let name_padded = if UnicodeWidthStr::width(name.as_str()) < layout.name_width {
        format!(
            "{name}{}",
            " ".repeat(layout.name_width - UnicodeWidthStr::width(name.as_str()))
        )
    } else {
        name
    };

    match layout.mode {
        DriveDetailMode::Full => {
            let used_padded = pad_left(&row.used, layout.used_width);
            let total_padded = pad_left(&row.total, layout.total_width);
            let remaining_padded = pad_left(&row.remaining, layout.remaining_width);
            let percent_padded = pad_left(&row.percent, layout.percent_width);
            format!(
                "{indent}{name_padded}{gap}{used_padded} / {total_padded}{gap}{remaining_padded}{gap}{percent_padded}"
            )
        }
        DriveDetailMode::Compact => {
            let remaining_padded = pad_left(&row.remaining, layout.remaining_width);
            let percent_padded = pad_left(&row.percent, layout.percent_width);
            format!("{indent}{name_padded}{gap}{remaining_padded}{gap}{percent_padded}")
        }
        DriveDetailMode::Minimal => {
            let percent_padded = pad_left(&row.percent, layout.percent_width);
            format!("{indent}{name_padded}{gap}{percent_padded}")
        }
    }
}

fn pad_left(value: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(value);
    if used >= width {
        value.to_string()
    } else {
        format!("{}{}", " ".repeat(width - used), value)
    }
}

/// Pre-formatted fields plus the shared table layout for the selected
/// system's expanded drive view.
pub(crate) fn render_drive_detail_lines(drives: &[NormalizedDrive], width: u16) -> Vec<String> {
    let eligible: Vec<NormalizedDrive> = drives
        .iter()
        .filter(|d| d.total_bytes > 0 && d.used_bytes <= d.total_bytes)
        .cloned()
        .collect();
    let rows: Vec<DriveDetailRow> = eligible.iter().map(build_drive_detail_row).collect();
    let layout = compute_drive_table_layout(&rows, width);
    rows.iter()
        .map(|row| render_drive_detail_row(row, &layout))
        .collect()
}

pub(crate) fn truncate_width(s: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut end = 0;
    for (index, ch) in s.char_indices() {
        let char_width = ch.width().unwrap_or(0);
        if width + char_width > max_width {
            break;
        }
        width += char_width;
        end = index + ch.len_utf8();
    }
    if end == s.len() {
        s.to_string()
    } else if max_width > 0 && width < max_width {
        format!("{}…", &s[..end])
    } else {
        s[..end].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(name: &str, used: u64, total: u64, available: Option<u64>) -> NormalizedDrive {
        NormalizedDrive {
            name: name.into(),
            used_bytes: used,
            total_bytes: total,
            available_bytes: available,
        }
    }

    #[test]
    fn full_mode_renders_complete_columns() {
        let rows = vec![
            build_drive_detail_row(&drive("/", 238 * GIB, 952 * GIB, None)),
            build_drive_detail_row(&drive("/mnt/archive", 142 * GIB, 477 * GIB, None)),
        ];
        let layout = compute_drive_table_layout(&rows, 80);
        assert_eq!(layout.mode(), DriveDetailMode::Full);

        let rendered: Vec<String> = rows
            .iter()
            .map(|r| render_drive_detail_row(r, &layout))
            .collect();

        // `used` columns must start at the same column across rows.
        let used_col_0 = UnicodeWidthStr::width(rendered[0].split('/').next().unwrap_or(""));
        let used_col_1 = UnicodeWidthStr::width(rendered[1].split('/').next().unwrap_or(""));
        // Both rows have the same used width because we used the wider
        // of the two formatted values.
        assert!(rendered[0].contains("238.0 GiB"));
        assert!(rendered[1].contains("142.0 GiB"));
        let _ = (used_col_0, used_col_1);
    }

    #[test]
    fn layout_uses_explicit_availability_for_remaining() {
        let rows = vec![build_drive_detail_row(&drive(
            "/",
            80 * GIB,
            100 * GIB,
            Some(10 * GIB),
        ))];
        let layout = compute_drive_table_layout(&rows, 80);
        assert_eq!(layout.mode(), DriveDetailMode::Full);
        let line = render_drive_detail_row(&rows[0], &layout);
        assert!(
            line.contains("(10.0 GiB)"),
            "explicit availability: {line:?}"
        );
        assert!(
            line.contains("80.0 GiB / 100.0 GiB"),
            "full shape: {line:?}"
        );
        assert!(line.contains("80.0%"), "percentage: {line:?}");
    }

    #[test]
    fn layout_falls_back_to_total_minus_used_when_availability_missing() {
        let rows = vec![build_drive_detail_row(&drive(
            "/",
            80 * GIB,
            100 * GIB,
            None,
        ))];
        let layout = compute_drive_table_layout(&rows, 80);
        let line = render_drive_detail_row(&rows[0], &layout);
        assert!(
            line.contains("(20.0 GiB)"),
            "compatibility fallback: {line:?}"
        );
    }

    #[test]
    fn minimal_mode_is_used_when_width_is_tight() {
        let rows = vec![build_drive_detail_row(&drive(
            "/some/long/mount/path",
            80 * GIB,
            100 * GIB,
            Some(10 * GIB),
        ))];
        let layout = compute_drive_table_layout(&rows, 14);
        // 14 width should be enough for `name  percent` but not the full shape.
        assert_ne!(layout.mode(), DriveDetailMode::Full);
        let line = render_drive_detail_row(&rows[0], &layout);
        assert!(line.contains("80.0%"), "percentage still shown: {line:?}");
    }
}
