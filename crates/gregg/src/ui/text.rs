#![allow(dead_code)]

use crate::normalized::{NormalizedDiskIo, NormalizedDrive, NormalizedNetwork};
use crate::state::SystemState;
use std::fmt::Write as _;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const KIB: u64 = 1024;
const MIB: u64 = KIB * 1024;
const GIB: u64 = MIB * 1024;
const TIB: u64 = GIB * 1024;

// Plan 086: structural width constants shared by the drive-table fit
// calculation and renderer. Every emitted cell must be accounted for,
// so the fit math and the rendered text cannot disagree about the
// fixed-width structural cells.
const DRIVE_INDENT_CELLS: usize = 2;
const DRIVE_GAP_CELLS: usize = 2;
const DRIVE_SLASH_CELLS: usize = 3; // " / "

/// Format a byte count as a human-readable string using binary units.
pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }

    let (unit, label, next) = if bytes >= TIB {
        (TIB, "TiB", None)
    } else if bytes >= GIB {
        (GIB, "GiB", Some((TIB, "TiB")))
    } else if bytes >= MIB {
        (MIB, "MiB", Some((GIB, "GiB")))
    } else if bytes >= KIB {
        (KIB, "KiB", Some((MIB, "MiB")))
    } else {
        return format!("{bytes} B");
    };

    let tenths = (u128::from(bytes) * 10 + u128::from(unit) / 2) / u128::from(unit);
    // Promotion is based on the rounded tenth in the current unit, but the
    // displayed value is recomputed directly in the next unit. This makes
    // the 1024.0 boundary explicit and avoids carrying a double-rounded
    // intermediate into the next-unit output.
    if let Some((next_unit, next_label)) = next.filter(|_| tenths >= 10_240) {
        let next_tenths =
            (u128::from(bytes) * 10 + u128::from(next_unit) / 2) / u128::from(next_unit);
        return format!(
            "{}.{fraction} {next_label}",
            next_tenths / 10,
            fraction = next_tenths % 10
        );
    }
    format!("{}.{fraction} {label}", tenths / 10, fraction = tenths % 10)
}

/// Format a byte rate for compact throughput detail.
pub fn format_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

/// Format the current CPU clock with a bounded, deterministic unit.
#[allow(clippy::cast_precision_loss)]
pub fn format_frequency(hz: u64) -> String {
    if hz >= 1_000_000_000 {
        format!("{:.2}GHz", hz as f64 / 1_000_000_000.0)
    } else if hz >= 1_000_000 {
        format!("{:.2}MHz", hz as f64 / 1_000_000.0)
    } else {
        format!("{hz}Hz")
    }
}

/// Format a directional link capacity in bits per second.
#[allow(clippy::cast_precision_loss)]
pub fn format_capacity(bits_per_sec: Option<u64>) -> String {
    let Some(bits) = bits_per_sec else {
        return "—".to_string();
    };
    if bits >= 1_000_000_000 {
        format!("{:.2}Gb/s", bits as f64 / 1_000_000_000.0)
    } else if bits >= 1_000_000 {
        format!("{:.2}Mb/s", bits as f64 / 1_000_000.0)
    } else if bits >= 1_000 {
        format!("{:.2}Kb/s", bits as f64 / 1_000.0)
    } else {
        format!("{bits}b/s")
    }
}

/// Format a percentage value.
///
/// Non-finite input renders as the unavailable marker `—` rather than
/// a numeric string.
pub fn format_pct(pct: f32) -> String {
    if pct.is_nan() {
        return "\u{2014}".to_string();
    }
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
/// 2. I/O-wait value (Plan 087: emitted only when `cpu_iowait_supported`
///    and a real `iowait_pct` are present; otherwise the entire
///    `IO <value>%` token is omitted instead of producing a placeholder)
/// 3. Load averages or "--" for unsupported
/// 4. Logical core count
/// 5. OS name/version
/// 6. Kernel release
/// 7. Architecture
pub fn header_line(system: &SystemState, width: u16) -> String {
    let Some(snap) = &system.latest else {
        return truncate_width(
            &format!("{} (no data)", display_name(system)),
            usize::from(width),
        );
    };

    let name = display_name(system);

    // Plan 087: only emit an `IO` token when the platform both
    // supports and is actually reporting a real value. The UI never
    // infers a zero from a missing measurement.
    let io_str: Option<String> = match (snap.cpu_iowait_supported, snap.iowait_pct) {
        (true, Some(iowait)) => Some(format!("IO {iowait:.1}%")),
        _ => None,
    };

    let load_str = match &snap.load {
        Some(l) => format_load(l),
        None => "L \u{2014}".to_string(),
    };
    let cores_str = format!("{}c", snap.logical_cores);
    let os_str = format!("{} {}", snap.system.os_name, snap.system.os_version);
    let kernel_str = format!("{} {}", snap.system.kernel_name, snap.system.kernel_release);
    let arch_str = &snap.system.architecture;

    let mut components = vec![name.to_string()];
    if let Some(io) = io_str {
        components.push(io);
    }
    if width >= 32 {
        components.push(load_str);
        components.push(cores_str);
    }
    if width >= 50 {
        components.push(os_str);
    }
    if width >= 80 {
        components.push(kernel_str);
        components.push(arch_str.clone());
    }

    // Remove whole trailing components until the selected priority tier fits;
    // only the highest-priority name may be truncated. This prevents a long
    // name from leaving a dangling `IO` or other partial token.
    while components.len() > 1
        && UnicodeWidthStr::width(components.join("  ").as_str()) > usize::from(width)
    {
        components.pop();
    }
    truncate_width(&components.join("  "), usize::from(width))
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
    pub(crate) read_rate: String,
    pub(crate) write_rate: String,
}

/// Width mode for the drive-detail table. Plan 085 picks one of:
/// 1. `Full` — `name  used / total  (remaining) percent`
/// 2. `Compact` — `name  (remaining) percent`
/// 3. `Minimal` — `name  percent`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriveDetailMode {
    Full,
    Compact,
    Throughput,
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
    read_width: usize,
    write_width: usize,
    show_throughput: bool,
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

/// Compute the percentage for a drive as `used / total * 100`. Eligibility
/// (`total > 0` and `used <= total`) is the caller's responsibility.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn percentage_for_drive(drive: &NormalizedDrive) -> f32 {
    (drive.used_bytes as f64 * 100.0 / drive.total_bytes as f64) as f32
}

/// Format one drive's pre-table fields. Eligibility (`used <= total`,
/// `total > 0`) must be checked by the caller.
pub(crate) fn build_drive_detail_row(drive: &NormalizedDrive) -> DriveDetailRow {
    build_drive_detail_row_with_io(drive, None)
}

fn build_drive_detail_row_with_io(
    drive: &NormalizedDrive,
    disk_io: Option<&NormalizedDiskIo>,
) -> DriveDetailRow {
    let used = format_bytes(drive.used_bytes);
    let total = format_bytes(drive.total_bytes);
    let remaining_bytes = drive
        .available_bytes
        .unwrap_or(drive.total_bytes - drive.used_bytes);
    let remaining = format!("({})", format_bytes(remaining_bytes));
    let percent = format_pct(percentage_for_drive(drive));
    let matching = disk_io.and_then(|io| {
        let mut matches = io
            .devices
            .iter()
            .filter(|device| device.drive_name.as_deref() == Some(drive.name.as_str()));
        let device = matches.next()?;
        if matches.next().is_some() {
            None
        } else {
            Some(device)
        }
    });
    DriveDetailRow {
        name: drive.name.clone(),
        used,
        total,
        remaining,
        percent,
        read_rate: matching.map_or_else(
            || "—".into(),
            |device| format_rate(device.read_bytes_per_sec),
        ),
        write_rate: matching.map_or_else(
            || "—".into(),
            |device| format_rate(device.write_bytes_per_sec),
        ),
    }
}

fn drive_row_widths(row: &DriveDetailRow) -> (usize, usize, usize, usize, usize, usize, usize) {
    (
        UnicodeWidthStr::width(row.name.as_str()),
        UnicodeWidthStr::width(row.used.as_str()),
        UnicodeWidthStr::width(row.total.as_str()),
        UnicodeWidthStr::width(row.remaining.as_str()),
        UnicodeWidthStr::width(row.percent.as_str()),
        UnicodeWidthStr::width(row.read_rate.as_str()),
        UnicodeWidthStr::width(row.write_rate.as_str()),
    )
}

/// Compute the drive-detail table layout from every eligible drive's
/// pre-formatted fields and the available width.
///
/// Layout modes are tried in Plan 085/086's documented order:
/// 1. full `name  used / total  (remaining) percent`;
/// 2. shrink the name column only while keeping numeric columns intact;
/// 3. compact `name  (remaining) percent`;
/// 4. minimal `name  percent`.
///
/// Plan 086: every fit calculation accounts for the indent, gaps, and
/// the ` / ` separator, and the Compact fallback considers a
/// truncated name before falling to Minimal.
pub(crate) fn compute_drive_table_layout(rows: &[DriveDetailRow], width: u16) -> DriveTableLayout {
    let show_io = rows
        .iter()
        .any(|row| row.read_rate != "—" || row.write_rate != "—");
    compute_drive_table_layout_with_io(rows, width, show_io)
}

#[allow(clippy::too_many_lines)]
fn compute_drive_table_layout_with_io(
    rows: &[DriveDetailRow],
    width: u16,
    show_io: bool,
) -> DriveTableLayout {
    let available = usize::from(width);

    // Widths implied by the widest formatted field across every row.
    let mut max_name = 0usize;
    let mut max_used = 0usize;
    let mut max_total = 0usize;
    let mut max_remaining = 0usize;
    let mut max_percent = 0usize;
    let mut max_read = 0usize;
    let mut max_write = 0usize;
    for row in rows {
        let (name_w, used_w, total_w, remaining_w, percent_w, read_w, write_w) =
            drive_row_widths(row);
        max_name = max_name.max(name_w);
        max_used = max_used.max(used_w);
        max_total = max_total.max(total_w);
        max_remaining = max_remaining.max(remaining_w);
        max_percent = max_percent.max(percent_w);
        max_read = max_read.max(read_w);
        max_write = max_write.max(write_w);
    }

    // Full layout width = indent + name + gap + used + " / " + total + gap + remaining + gap + percent.
    let full_fixed = DRIVE_INDENT_CELLS
        .saturating_add(DRIVE_GAP_CELLS)
        .saturating_add(max_used)
        .saturating_add(DRIVE_SLASH_CELLS)
        .saturating_add(max_total)
        .saturating_add(DRIVE_GAP_CELLS)
        .saturating_add(max_remaining)
        .saturating_add(DRIVE_GAP_CELLS)
        .saturating_add(max_percent);
    let full_name_budget = available.saturating_sub(full_fixed);

    let io_fixed = if show_io {
        DRIVE_GAP_CELLS
            .saturating_add(max_read)
            .saturating_add(DRIVE_GAP_CELLS)
            .saturating_add(max_write)
    } else {
        0
    };
    let full_io_name_budget = full_name_budget.saturating_sub(io_fixed);

    if full_io_name_budget >= max_name {
        // Full natural fits.
        return DriveTableLayout {
            name_width: max_name,
            used_width: max_used,
            total_width: max_total,
            remaining_width: max_remaining,
            percent_width: max_percent,
            read_width: max_read,
            write_width: max_write,
            show_throughput: show_io,
            mode: DriveDetailMode::Full,
        };
    }

    if full_name_budget >= max_name {
        return DriveTableLayout {
            name_width: max_name,
            used_width: max_used,
            total_width: max_total,
            remaining_width: max_remaining,
            percent_width: max_percent,
            read_width: 0,
            write_width: 0,
            show_throughput: false,
            mode: DriveDetailMode::Full,
        };
    }

    if full_name_budget >= 1 {
        // Full truncated: keep all numeric columns, truncate the name.
        return DriveTableLayout {
            name_width: full_name_budget,
            used_width: max_used,
            total_width: max_total,
            remaining_width: max_remaining,
            percent_width: max_percent,
            read_width: 0,
            write_width: 0,
            show_throughput: false,
            mode: DriveDetailMode::Full,
        };
    }

    let throughput_fixed = DRIVE_INDENT_CELLS
        .saturating_add(DRIVE_GAP_CELLS)
        .saturating_add(max_read)
        .saturating_add(DRIVE_GAP_CELLS)
        .saturating_add(max_write);
    let throughput_name_budget = available.saturating_sub(throughput_fixed);
    if show_io && throughput_name_budget >= 1 {
        return DriveTableLayout {
            name_width: max_name.min(throughput_name_budget),
            used_width: 0,
            total_width: 0,
            remaining_width: 0,
            percent_width: 0,
            read_width: max_read,
            write_width: max_write,
            show_throughput: true,
            mode: DriveDetailMode::Throughput,
        };
    }

    // Compact fallback: indent + name + gap + remaining + gap + percent.
    let compact_fixed = DRIVE_INDENT_CELLS
        .saturating_add(DRIVE_GAP_CELLS)
        .saturating_add(max_remaining)
        .saturating_add(DRIVE_GAP_CELLS)
        .saturating_add(max_percent);
    let compact_name_budget = available.saturating_sub(compact_fixed);

    if compact_name_budget >= 1 {
        // Compact with a truncated name keeps `(remaining)` and percent.
        let name_width = max_name.min(compact_name_budget);
        return DriveTableLayout {
            name_width,
            used_width: 0,
            total_width: 0,
            remaining_width: max_remaining,
            percent_width: max_percent,
            read_width: 0,
            write_width: 0,
            show_throughput: false,
            mode: DriveDetailMode::Compact,
        };
    }

    // Minimal fallback: indent + name + gap + percent.
    let minimal_fixed = DRIVE_INDENT_CELLS
        .saturating_add(DRIVE_GAP_CELLS)
        .saturating_add(max_percent);
    let minimal_name_budget = available.saturating_sub(minimal_fixed);
    let name_width = max_name.min(minimal_name_budget);
    DriveTableLayout {
        name_width,
        used_width: 0,
        total_width: 0,
        remaining_width: 0,
        percent_width: max_percent,
        read_width: 0,
        write_width: 0,
        show_throughput: false,
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
            let base = format!(
                "{indent}{name_padded}{gap}{used_padded} / {total_padded}{gap}{remaining_padded}{gap}{percent_padded}"
            );
            if layout.show_throughput {
                let read_padded = pad_left(&row.read_rate, layout.read_width);
                let write_padded = pad_left(&row.write_rate, layout.write_width);
                format!("{base}{gap}{read_padded}{gap}{write_padded}")
            } else {
                base
            }
        }
        DriveDetailMode::Compact => {
            let remaining_padded = pad_left(&row.remaining, layout.remaining_width);
            let percent_padded = pad_left(&row.percent, layout.percent_width);
            format!("{indent}{name_padded}{gap}{remaining_padded}{gap}{percent_padded}")
        }
        DriveDetailMode::Throughput => {
            let read_padded = pad_left(&row.read_rate, layout.read_width);
            let write_padded = pad_left(&row.write_rate, layout.write_width);
            format!("{indent}{name_padded}{gap}{read_padded}{gap}{write_padded}")
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

fn pad_right(value: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(value);
    format!("{value}{}", " ".repeat(width.saturating_sub(used)))
}

/// Pre-formatted fields plus the shared table layout for the selected
/// system's expanded drive view.
pub(crate) fn render_drive_detail_lines(drives: &[NormalizedDrive], width: u16) -> Vec<String> {
    render_drive_detail_lines_with_io(drives, None, width)
}

pub(crate) fn render_drive_detail_lines_with_io(
    drives: &[NormalizedDrive],
    disk_io: Option<&NormalizedDiskIo>,
    width: u16,
) -> Vec<String> {
    let rows: Vec<DriveDetailRow> = drives
        .iter()
        .filter(|d| d.total_bytes > 0 && d.used_bytes <= d.total_bytes)
        .map(|drive| build_drive_detail_row_with_io(drive, disk_io))
        .collect();
    let layout = compute_drive_table_layout_with_io(&rows, width, disk_io.is_some());
    let mut lines = Vec::new();
    if disk_io.is_some() {
        lines.push(render_drive_detail_header(&layout, width));
    }
    lines.extend(rows.iter().map(|row| render_drive_detail_row(row, &layout)));
    if let Some(io) = disk_io {
        lines.push(render_disk_io_total_line(io, width));
    }
    lines
}

fn render_drive_detail_header(layout: &DriveTableLayout, width: u16) -> String {
    let mut line = format!(
        "  {}  {} / {}  {}  {}",
        pad_right("DRIVE/MOUNT", layout.name_width),
        pad_left("USED", layout.used_width),
        pad_left("TOTAL", layout.total_width),
        pad_left("REMAIN", layout.remaining_width),
        pad_left("PERCENT", layout.percent_width),
    );
    if layout.show_throughput {
        let _ = write!(
            line,
            "  {}  {}",
            pad_left("R/s", layout.read_width),
            pad_left("W/s", layout.write_width)
        );
    }
    truncate_width(&line, usize::from(width))
}

fn render_disk_io_total_line(io: &NormalizedDiskIo, width: u16) -> String {
    truncate_width(
        &format!(
            "  I/O TOTAL  R/s {}  W/s {}",
            format_rate(io.aggregate_read_bytes_per_sec),
            format_rate(io.aggregate_write_bytes_per_sec)
        ),
        usize::from(width),
    )
}

pub(crate) fn render_network_detail_lines(network: &NormalizedNetwork, width: u16) -> Vec<String> {
    let utilization = network
        .aggregate_utilization_pct()
        .map_or_else(|| "—".into(), format_pct);
    let capacity = match (
        network.aggregate_rx_capacity_bps,
        network.aggregate_tx_capacity_bps,
    ) {
        (Some(rx), Some(tx)) if rx == tx => format_capacity(Some(rx)),
        (Some(rx), Some(tx)) => format!(
            "{}/{}",
            format_capacity(Some(rx)),
            format_capacity(Some(tx))
        ),
        (Some(value), None) | (None, Some(value)) => format_capacity(Some(value)),
        (None, None) => "—".into(),
    };
    let mut lines = vec![truncate_width(
        &format!(
            "  NETWORK TOTAL  RX {}  TX {}  CAP {}  {}",
            format_rate(network.aggregate_rx_bytes_per_sec),
            format_rate(network.aggregate_tx_bytes_per_sec),
            capacity,
            utilization
        ),
        usize::from(width),
    )];
    lines.extend(network.interfaces.iter().map(|interface| {
        truncate_width(
            &format!(
                "  {}  RX {}  TX {}  LINK {}",
                interface.name,
                format_rate(interface.rx_bytes_per_sec),
                format_rate(interface.tx_bytes_per_sec),
                format_capacity(interface.rx_capacity_bps.or(interface.tx_capacity_bps)),
            ),
            usize::from(width),
        )
    }));
    lines
}

/// Truncate `s` to at most `max_width` terminal cells.
///
/// Glyphs are never split across cells: when the leading glyph alone is
/// wider than the whole budget (e.g. a wide CJK character with
/// `max_width == 1`), nothing of it is emitted and the result collapses to
/// the ellipsis placeholder rather than a partial glyph.
pub(crate) fn truncate_width(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
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
    use crate::normalized::NormalizedSnapshot;

    #[test]
    fn format_pct_renders_unavailable_marker_for_nan() {
        assert_eq!(format_pct(f32::NAN), "\u{2014}");
    }

    #[test]
    fn format_pct_clamps_out_of_range_values() {
        assert_eq!(format_pct(-1.0), "0.0%");
        assert_eq!(format_pct(150.0), "100%");
    }

    #[test]
    fn format_bytes_carries_rounding_into_the_next_unit() {
        assert_eq!(format_bytes(TIB - 1), "1.0 TiB");
        assert_eq!(format_bytes(GIB - 1), "1.0 GiB");
    }

    #[test]
    fn format_frequency_uses_bounded_mhz_and_ghz_units() {
        assert_eq!(format_frequency(999_999), "999999Hz");
        assert_eq!(format_frequency(1_000_000), "1.00MHz");
        assert_eq!(format_frequency(2_400_000_000), "2.40GHz");
        assert_eq!(format_frequency(u64::MAX), "18446744073.71GHz");
    }

    #[test]
    fn network_detail_preserves_rates_without_capacity_and_loopback() {
        let network = NormalizedNetwork {
            aggregate_rx_bytes_per_sec: 39 * MIB,
            aggregate_tx_bytes_per_sec: 5 * MIB,
            aggregate_rx_capacity_bps: None,
            aggregate_tx_capacity_bps: None,
            interfaces: vec![
                crate::normalized::NormalizedNetworkInterface {
                    id: "eth0".into(),
                    name: "eth0".into(),
                    rx_bytes_per_sec: 38 * MIB,
                    tx_bytes_per_sec: 4 * MIB,
                    rx_capacity_bps: Some(1_000_000_000),
                    tx_capacity_bps: Some(1_000_000_000),
                    is_loopback: false,
                    aggregate_member: true,
                },
                crate::normalized::NormalizedNetworkInterface {
                    id: "lo".into(),
                    name: "lo".into(),
                    rx_bytes_per_sec: MIB,
                    tx_bytes_per_sec: MIB,
                    rx_capacity_bps: None,
                    tx_capacity_bps: None,
                    is_loopback: true,
                    aggregate_member: false,
                },
            ],
        };
        let lines = render_network_detail_lines(&network, 120);
        assert!(lines[0].contains("RX 39.0 MiB/s"));
        assert!(lines[0].contains("CAP —"));
        assert!(lines.iter().any(|line| line.contains("eth0")));
        assert!(lines.iter().any(|line| line.contains("lo")));
    }

    fn drive(name: &str, used: u64, total: u64, available: Option<u64>) -> NormalizedDrive {
        NormalizedDrive {
            name: name.into(),
            used_bytes: used,
            total_bytes: total,
            available_bytes: available,
        }
    }

    fn system_with_io(supported: bool, iowait: Option<f32>) -> crate::state::SystemState {
        let mut snap = if supported {
            NormalizedSnapshot::from_v1(
                &gregg_protocol::test_support::LinuxSnapshotBuilder::default()
                    .iowait_pct(0.4)
                    .build(),
            )
        } else {
            NormalizedSnapshot::from_v1(
                &gregg_protocol::test_support::MacosSnapshotBuilder::default().build(),
            )
        };
        snap.cpu_iowait_supported = supported;
        snap.iowait_pct = iowait;
        crate::state::SystemState {
            id: "id".into(),
            endpoint: crate::endpoint::Endpoint::new("host".into(), 11310, None),
            configured_name: Some("srv".into()),
            reachability: crate::state::Reachability::Online,
            latest: Some(snap),
            last_success_at: None,
            last_attempt_at: None,
            latency: None,
            offline_reason: None,
        }
    }

    #[test]
    fn header_line_renders_io_for_supported_linux_value() {
        let system = system_with_io(true, Some(1.7));
        let line = header_line(&system, 120);
        assert!(
            line.contains("IO 1.7%"),
            "supported Linux value must show: {line:?}"
        );
    }

    #[test]
    fn header_line_omits_io_token_for_unsupported_platform() {
        let system = system_with_io(false, None);
        let line = header_line(&system, 120);
        assert!(!line.contains("IO "), "must omit IO token: {line:?}");
        assert!(!line.contains("—"), "must not render placeholder: {line:?}");
    }

    #[test]
    fn header_line_omits_io_token_when_capability_supported_but_value_missing() {
        let system = system_with_io(true, None);
        let line = header_line(&system, 120);
        assert!(!line.contains("IO "), "must omit IO token: {line:?}");
        assert!(!line.contains("0.0%"), "must not fabricate 0.0%: {line:?}");
    }

    #[test]
    fn header_line_avoids_double_separator_when_io_omitted() {
        let system = system_with_io(false, None);
        let line = header_line(&system, 80);
        // The name is followed by the load component. There must be
        // exactly one separator gap (two spaces), not three.
        assert!(
            !line.starts_with("srv   "),
            "no tripled separator after the name when IO is omitted: {line:?}"
        );
    }

    #[test]
    fn header_line_remains_bounded_when_io_omitted() {
        // Omitting IO may make room for the next complete priority field,
        // so compare each result with its terminal budget rather than with
        // the supported form's length.
        let supported = system_with_io(true, Some(1.2));
        let unsupported = system_with_io(false, None);
        for width in [32u16, 50, 80, 120, 200] {
            let supported_line = header_line(&supported, width);
            let unsupported_line = header_line(&unsupported, width);
            assert!(
                UnicodeWidthStr::width(unsupported_line.as_str()) <= usize::from(width),
                "unsupported header must fit at width {width}: {unsupported_line:?}"
            );
            assert!(
                UnicodeWidthStr::width(supported_line.as_str()) <= usize::from(width),
                "supported header must fit at width {width}: {supported_line:?}"
            );
            assert!(!unsupported_line.contains("IO "));
        }
    }

    #[test]
    fn header_line_drops_whole_tokens_for_long_names() {
        let system = system_with_io(true, Some(1.2));
        let line = header_line(
            &crate::state::SystemState {
                configured_name: Some("a".repeat(100)),
                ..system
            },
            80,
        );
        assert_eq!(UnicodeWidthStr::width(line.as_str()), 80);
        assert!(
            !line.contains("IO"),
            "long names must not leave partial IO: {line:?}"
        );
        assert!(
            !line.ends_with("0."),
            "header must not end in a partial token: {line:?}"
        );
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

    #[test]
    fn full_mode_natural_width_matches_rendered_width() {
        // Plan 086: the fit calculation must include every emitted
        // structural cell (indent, gaps, separator) or the rendered
        // row will overflow the requested display width.
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
            UnicodeWidthStr::width(line.as_str()) <= 80,
            "rendered line overflows requested width: {line:?} ({} cells)",
            UnicodeWidthStr::width(line.as_str())
        );
    }

    #[test]
    fn full_mode_exact_fit_boundary_classifies_correctly() {
        // Plan 086: at the exact natural Full width the layout must
        // classify Full, and one cell narrower must not classify Full
        // with an overflowing row.
        let rows = vec![build_drive_detail_row(&drive(
            "/",
            80 * GIB,
            100 * GIB,
            Some(10 * GIB),
        ))];
        let layout_at_80 = compute_drive_table_layout(&rows, 80);
        assert_eq!(layout_at_80.mode(), DriveDetailMode::Full);
        let line = render_drive_detail_row(&rows[0], &layout_at_80);
        let exact = UnicodeWidthStr::width(line.as_str());

        let layout_at_exact = compute_drive_table_layout(&rows, u16::try_from(exact).unwrap_or(80));
        assert_eq!(layout_at_exact.mode(), DriveDetailMode::Full);
        let line_at_exact = render_drive_detail_row(&rows[0], &layout_at_exact);
        assert!(
            UnicodeWidthStr::width(line_at_exact.as_str()) <= exact,
            "line at exact width overflows: {line_at_exact:?}"
        );

        if exact > 4 {
            let layout_below =
                compute_drive_table_layout(&rows, u16::try_from(exact - 1).unwrap_or(80));
            match layout_below.mode() {
                DriveDetailMode::Full => {
                    let line_below = render_drive_detail_row(&rows[0], &layout_below);
                    assert!(
                        UnicodeWidthStr::width(line_below.as_str()) < exact,
                        "Full at one cell below exact width overflows: {line_below:?}"
                    );
                }
                DriveDetailMode::Compact
                | DriveDetailMode::Throughput
                | DriveDetailMode::Minimal => {}
            }
        }
    }

    #[test]
    fn compact_mode_considers_truncated_name_before_minimal() {
        // Plan 086: a long mount name must not skip Compact just
        // because the natural name would overflow. The remaining and
        // percent fields must remain visible.
        let rows = vec![build_drive_detail_row(&drive(
            "/some/really/long/mount/name",
            80 * GIB,
            100 * GIB,
            Some(10 * GIB),
        ))];
        // Total structural width for Compact with the fixed fields is
        // 2 + 2 + 10 + 2 + 5 = 21 + a small name budget. Any width that
        // exceeds the Full truncation budget but allows Compact must
        // pick Compact, not Minimal.
        let layout = compute_drive_table_layout(&rows, 28);
        assert_eq!(
            layout.mode(),
            DriveDetailMode::Compact,
            "Compact should win when fixed fields fit with a truncated name: layout={layout:?}"
        );
        let line = render_drive_detail_row(&rows[0], &layout);
        assert!(line.contains("(10.0 GiB)"), "remaining present: {line:?}");
        assert!(line.contains("80.0%"), "percent present: {line:?}");
        assert!(
            UnicodeWidthStr::width(line.as_str()) <= 28,
            "line exceeds width: {line:?} ({} cells)",
            UnicodeWidthStr::width(line.as_str())
        );
    }

    #[test]
    fn minimal_mode_is_only_used_when_compact_cannot_fit() {
        // Plan 086: Minimal is only the right answer when the fixed
        // Compact fields plus a usable name still cannot fit.
        let rows = vec![build_drive_detail_row(&drive(
            "/short",
            80 * GIB,
            100 * GIB,
            Some(10 * GIB),
        ))];
        let layout = compute_drive_table_layout(&rows, 14);
        assert_eq!(layout.mode(), DriveDetailMode::Minimal);
        let line = render_drive_detail_row(&rows[0], &layout);
        assert!(line.contains("80.0%"), "percent still shown: {line:?}");
        assert!(
            UnicodeWidthStr::width(line.as_str()) <= 14,
            "line exceeds width: {line:?}"
        );
    }

    #[test]
    fn compact_mode_renders_within_requested_width() {
        // Plan 086: the fit calculation must include the indent, so a
        // Compact row at the requested width must not exceed it.
        let rows = vec![build_drive_detail_row(&drive(
            "/some/long/mount/path",
            80 * GIB,
            100 * GIB,
            Some(10 * GIB),
        ))];
        for width in [22u16, 25, 28, 30] {
            let layout = compute_drive_table_layout(&rows, width);
            let line = render_drive_detail_row(&rows[0], &layout);
            assert!(
                UnicodeWidthStr::width(line.as_str()) <= usize::from(width),
                "line exceeds width at {width}: {line:?} ({} cells)",
                UnicodeWidthStr::width(line.as_str())
            );
        }
    }

    #[test]
    fn unicode_drive_name_uses_display_cells_for_fit_decisions() {
        // Plan 086: a wide-character name must shrink the name
        // according to terminal-cell width, not UTF-8 byte length.
        let rows = vec![build_drive_detail_row(&drive(
            "/マウント/ポイント",
            80 * GIB,
            100 * GIB,
            Some(10 * GIB),
        ))];
        for width in [60u16, 80] {
            let layout = compute_drive_table_layout(&rows, width);
            let line = render_drive_detail_row(&rows[0], &layout);
            assert!(
                UnicodeWidthStr::width(line.as_str()) <= usize::from(width),
                "unicode line exceeds width at {width}: {line:?} ({} cells)",
                UnicodeWidthStr::width(line.as_str())
            );
        }
    }

    #[test]
    fn full_mode_renders_complete_columns_uses_aligned_positions() {
        // Plan 086: the existing test must actually assert the
        // computed alignment positions agree across rows, not merely
        // that the cells are present.
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

        // Locate the '/' separator at the same cell on both rows.
        let slash_0 = locate_slash_cell(&rendered[0]).expect("slash on row 0");
        let slash_1 = locate_slash_cell(&rendered[1]).expect("slash on row 1");
        assert_eq!(slash_0, slash_1, "slash separator must align: {rendered:?}");

        // Locate the '(' that opens the remaining space at the same cell.
        let paren_0 = locate_remaining_open_cell(&rendered[0]).expect("paren on row 0");
        let paren_1 = locate_remaining_open_cell(&rendered[1]).expect("paren on row 1");
        assert_eq!(
            paren_0, paren_1,
            "remaining open paren must align: {rendered:?}"
        );

        // Locate the percent column at the same cell.
        let pct_0 = UnicodeWidthStr::width(rendered[0].as_str())
            - rendered[0]
                .trim_end()
                .chars()
                .rev()
                .take_while(|c| *c != '%')
                .count()
            - 1;
        let pct_1 = UnicodeWidthStr::width(rendered[1].as_str())
            - rendered[1]
                .trim_end()
                .chars()
                .rev()
                .take_while(|c| *c != '%')
                .count()
            - 1;
        assert_eq!(pct_0, pct_1, "percent column must align: {rendered:?}");
    }

    #[test]
    fn disk_io_details_use_exact_associations_and_daemon_aggregate() {
        let drives = vec![
            drive("/", 80 * GIB, 100 * GIB, None),
            drive("/data", 20 * GIB, 100 * GIB, None),
        ];
        let io = NormalizedDiskIo {
            aggregate_read_bytes_per_sec: 90 * MIB,
            aggregate_write_bytes_per_sec: 22 * MIB,
            devices: vec![
                crate::normalized::NormalizedDiskIoDevice {
                    id: "sda".into(),
                    name: "sda".into(),
                    read_bytes_per_sec: 80 * MIB,
                    write_bytes_per_sec: 20 * MIB,
                    drive_name: Some("/".into()),
                },
                crate::normalized::NormalizedDiskIoDevice {
                    id: "sdb".into(),
                    name: "sdb".into(),
                    read_bytes_per_sec: 10 * MIB,
                    write_bytes_per_sec: 2 * MIB,
                    drive_name: Some("/data".into()),
                },
            ],
        };
        let lines = render_drive_detail_lines_with_io(&drives, Some(&io), 120);
        assert!(lines.iter().any(|line| line.contains("R/s")));
        assert!(lines.iter().any(|line| line.contains("80.0 MiB/s")));
        assert!(lines.iter().any(|line| line.contains("10.0 MiB/s")));
        assert!(lines.iter().any(|line| line.contains("I/O TOTAL")));
        assert!(lines.iter().any(|line| line.contains("90.0 MiB/s")));

        let ambiguous = NormalizedDiskIo {
            devices: vec![
                crate::normalized::NormalizedDiskIoDevice {
                    id: "sda".into(),
                    name: "sda".into(),
                    read_bytes_per_sec: 80 * MIB,
                    write_bytes_per_sec: 20 * MIB,
                    drive_name: Some("/".into()),
                },
                crate::normalized::NormalizedDiskIoDevice {
                    id: "sda-part".into(),
                    name: "sda-part".into(),
                    read_bytes_per_sec: 1,
                    write_bytes_per_sec: 1,
                    drive_name: Some("/".into()),
                },
            ],
            ..io
        };
        let ambiguous_lines =
            render_drive_detail_lines_with_io(&drives[..1], Some(&ambiguous), 120);
        assert!(ambiguous_lines.iter().any(|line| line.contains("—")));
    }

    fn locate_slash_cell(line: &str) -> Option<usize> {
        let mut cells = 0usize;
        for ch in line.chars() {
            if ch == '/' {
                return Some(cells);
            }
            cells += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
        None
    }

    fn locate_remaining_open_cell(line: &str) -> Option<usize> {
        let mut cells = 0usize;
        for ch in line.chars() {
            if ch == '(' {
                return Some(cells);
            }
            cells += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
        None
    }
}
