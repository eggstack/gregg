#![allow(dead_code)]

use gregg_protocol::LoadAverage;

use crate::state::SystemState;

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
pub fn format_load(load: &LoadAverage) -> String {
    format!("{:.2}/{:.2}/{:.2}", load.one, load.five, load.fifteen)
}

/// Compose a priority-aware header line for an online system.
///
/// Priority (dropped as width decreases):
/// 1. Display name or hostname
/// 2. I/O-wait value or "—" for unsupported
/// 3. Load averages
/// 4. Logical core count with load
/// 5. OS name/version
/// 6. Kernel release
/// 7. Architecture
pub fn header_line(system: &SystemState, width: u16) -> String {
    let Some(snap) = &system.latest else {
        return format!("{} (no data)", display_name(system));
    };

    let name = display_name(system);

    let io_str = if snap.capabilities.cpu_iowait {
        match snap.cpu.iowait_pct {
            Some(iowait) => format!("IO {iowait:.1}%"),
            None => "IO —".to_string(),
        }
    } else {
        "IO —".to_string()
    };

    let load_str = format_load(&snap.load);
    let cores_str = format!("{}c", snap.cpu.logical_cores);
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
