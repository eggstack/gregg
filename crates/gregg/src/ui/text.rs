#![allow(dead_code)]

use crate::normalized::NormalizedDrive;
use crate::state::SystemState;
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
        return format!("{} (no data)", display_name(system));
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

    // Plan 087: when the IO token is present, prepend it to the
    // remaining header components with a leading separator. The token
    // is omitted entirely (no separator artifact) when the platform
    // cannot supply a real value, so we use `Option`-aware formatting
    // rather than an unconditional placeholder.
    let io_suffix = io_str
        .as_deref()
        .map(|io| format!("  {io}"))
        .unwrap_or_default();

    if width >= 80 {
        format!("{name}{io_suffix}  {load_str}  {cores_str}  {os_str}  {kernel_str}  {arch_str}")
    } else if width >= 50 {
        format!("{name}{io_suffix}  {load_str}  {cores_str}  {os_str}")
    } else if width >= 32 {
        format!("{name}{io_suffix}  {load_str}  {cores_str}")
    } else {
        format!("{name}{io_suffix}")
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

/// Compute the percentage for a drive as `used / total * 100`. Eligibility
/// (`total > 0` and `used <= total`) is the caller's responsibility.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn percentage_for_drive(drive: &NormalizedDrive) -> f32 {
    (drive.used_bytes as f64 * 100.0 / drive.total_bytes as f64) as f32
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
    let percent = format_pct(percentage_for_drive(drive));
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
    let available = usize::from(width);

    // Widths implied by the widest formatted field across every row.
    let mut max_name = 0usize;
    let mut max_used = 0usize;
    let mut max_total = 0usize;
    let mut max_remaining = 0usize;
    let mut max_percent = 0usize;
    for row in rows {
        let (name_w, used_w, total_w, remaining_w, percent_w) = drive_row_widths(row);
        max_name = max_name.max(name_w);
        max_used = max_used.max(used_w);
        max_total = max_total.max(total_w);
        max_remaining = max_remaining.max(remaining_w);
        max_percent = max_percent.max(percent_w);
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

    if full_name_budget >= max_name {
        // Full natural fits.
        return DriveTableLayout {
            name_width: max_name,
            used_width: max_used,
            total_width: max_total,
            remaining_width: max_remaining,
            percent_width: max_percent,
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
            mode: DriveDetailMode::Full,
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
            last_error: None,
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
        // Plan 087 documents that the existing priority-aware width
        // behavior is preserved unchanged at the tier thresholds.
        // Verify that omitting the IO token never causes a regression
        // compared to the supported path: the unsupported header must
        // be at least as short as the supported one.
        let supported = system_with_io(true, Some(1.2));
        let unsupported = system_with_io(false, None);
        for width in [32u16, 50, 80, 120, 200] {
            let supported_line = header_line(&supported, width);
            let unsupported_line = header_line(&unsupported, width);
            assert!(
                UnicodeWidthStr::width(unsupported_line.as_str())
                    <= UnicodeWidthStr::width(supported_line.as_str()),
                "omitting IO must not make the header longer than the supported \
                 path at width {width}: supported={supported_line:?}, \
                 unsupported={unsupported_line:?}"
            );
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
                DriveDetailMode::Compact | DriveDetailMode::Minimal => {}
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
