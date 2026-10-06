//! Windows source abstraction for testable metric collection.
//!
//! Production code calls native Windows APIs behind a narrow FFI module.
//! Tests inject [`MockWindowsSource`] to exercise edge cases without
//! depending on the host state.

#![allow(unsafe_code)]

use crate::error::{CollectError, CollectErrorKind};

/// Cumulative CPU time counters from `GetSystemTimes`.
///
/// Windows kernel time includes idle time. To compute busy time:
///
/// ```text
/// kernel_busy = kernel - idle
/// total = kernel + user
/// busy = total - idle = kernel_busy + user
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawCpuTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
}

impl RawCpuTimes {
    /// Total tick count accumulated by `GetSystemTimes`.
    ///
    /// `GetSystemTimes` reports the *kernel* time as time spent in the
    /// kernel **including idle time**, so `idle` is already a subset of
    /// `kernel` and must not be added again. Adding it would double-count
    /// idle and collapse `busy()` (and therefore `usage_pct`) toward zero.
    #[must_use]
    pub fn total(self) -> u64 {
        self.kernel.saturating_add(self.user)
    }

    /// Busy time: total - idle. `idle` is a subset of `total`, not a
    /// separate component, so no saturation is lost in practice.
    #[must_use]
    pub fn busy(self) -> u64 {
        self.total().saturating_sub(self.idle)
    }
}

/// Physical memory from `GlobalMemoryStatusEx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawPhysicalMemory {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Commit charge from `GetPerformanceInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawCommit {
    pub commit_total_pages: u64,
    pub commit_limit_pages: u64,
    pub page_size_bytes: u64,
}

/// System identity fields collected via Windows APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIdentity {
    pub hostname: String,
    pub os_version: String,
    pub architecture: String,
    pub logical_cores: u32,
    pub physical_memory_bytes: u64,
    /// Number of active processor groups. Single-group systems return 1.
    pub processor_group_count: u16,
}

/// Processor topology information for the host-size guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawProcessorTopology {
    pub active_logical_processors: u32,
    pub group_count: u16,
}

/// Owned logical-drive result from the Windows native source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLogicalDrive {
    pub root: String,
    pub drive_type: u32,
    pub total_bytes: u64,
    pub total_free_bytes: u64,
    pub available_bytes: u64,
}

/// Cumulative disk byte counters from `IOCTL_DISK_PERFORMANCE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDiskIo {
    pub id: String,
    pub name: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

/// Cumulative interface counters and link metadata from IP Helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawNetworkInterface {
    pub id: String,
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_capacity_bps: Option<u64>,
    pub tx_capacity_bps: Option<u64>,
    pub is_loopback: bool,
    pub operational: bool,
    pub aggregate_member: bool,
}

/// Abstraction over native Windows system queries.
///
/// Production code calls FFI; tests inject a mock to exercise edge cases
/// without depending on the host state.
pub trait WindowsSource: Send + Sync + std::fmt::Debug {
    /// Enumerate ready logical drives and query their total/free capacity.
    fn logical_drives(&self) -> Result<Vec<RawLogicalDrive>, CollectError>;
    /// Read cumulative CPU time counters from `GetSystemTimes`.
    fn cpu_times(&self) -> Result<RawCpuTimes, CollectError>;

    /// Read the number of active logical processors across all groups.
    fn active_processor_count(&self) -> Result<u32, CollectError>;

    /// Read the processor topology (group count and total logical count).
    fn processor_topology(&self) -> Result<RawProcessorTopology, CollectError>;

    /// Read physical memory statistics from `GlobalMemoryStatusEx`.
    fn physical_memory(&self) -> Result<RawPhysicalMemory, CollectError>;

    /// Read commit charge from `GetPerformanceInfo`.
    fn commit(&self) -> Result<RawCommit, CollectError>;

    /// Read system identity fields.
    fn identity(&self) -> Result<RawIdentity, CollectError>;
    /// Read current MHz from documented processor power information.
    fn cpu_frequency_hz(&self) -> Result<Option<u64>, CollectError>;
    /// Read cumulative per-disk byte counters.
    fn disk_io(&self) -> Result<Vec<RawDiskIo>, CollectError>;
    /// Read cumulative interface counters and link metadata.
    fn network_interfaces(&self) -> Result<Vec<RawNetworkInterface>, CollectError>;
}

/// Mock Windows source for unit tests. All fields are public so tests can
/// inject different values between successive calls.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct MockWindowsSource {
    pub drives: Vec<RawLogicalDrive>,
    pub cpu: RawCpuTimes,
    pub topology: RawProcessorTopology,
    pub memory: RawPhysicalMemory,
    pub commit: RawCommit,
    pub identity: RawIdentity,
    pub cpu_error: bool,
    pub topology_error: bool,
    pub memory_error: bool,
    pub commit_error: bool,
    pub identity_error: bool,
    pub drives_error: bool,
    /// When true, `cpu_times` increments `idle`, `kernel`, and `user` by a
    /// small delta on each call so successive samples produce a valid
    /// non-zero CPU interval.
    pub auto_increment_cpu: bool,
    pub cpu_call_count: std::sync::atomic::AtomicU32,
    pub cpu_frequency: Option<u64>,
    pub disk: Vec<RawDiskIo>,
    pub network: Vec<RawNetworkInterface>,
}

impl Clone for MockWindowsSource {
    fn clone(&self) -> Self {
        Self {
            drives: self.drives.clone(),
            cpu: self.cpu,
            topology: self.topology,
            memory: self.memory,
            commit: self.commit,
            identity: self.identity.clone(),
            cpu_error: self.cpu_error,
            topology_error: self.topology_error,
            memory_error: self.memory_error,
            commit_error: self.commit_error,
            identity_error: self.identity_error,
            drives_error: self.drives_error,
            auto_increment_cpu: self.auto_increment_cpu,
            cpu_call_count: std::sync::atomic::AtomicU32::new(
                self.cpu_call_count
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            cpu_frequency: self.cpu_frequency,
            disk: self.disk.clone(),
            network: self.network.clone(),
        }
    }
}

impl MockWindowsSource {
    /// Build a mock returning sensible default values.
    #[must_use]
    pub fn success() -> Self {
        Self {
            drives: vec![RawLogicalDrive {
                root: "C:\\".to_string(),
                drive_type: DRIVE_FIXED,
                total_bytes: 100,
                total_free_bytes: 25,
                available_bytes: 20,
            }],
            cpu: RawCpuTimes {
                idle: 8_000,
                kernel: 8_500,
                user: 1_000,
            },
            topology: RawProcessorTopology {
                active_logical_processors: 8,
                group_count: 1,
            },
            memory: RawPhysicalMemory {
                total_bytes: 16_000_000_000,
                available_bytes: 10_000_000_000,
            },
            commit: RawCommit {
                commit_total_pages: 200_000,
                commit_limit_pages: 800_000,
                page_size_bytes: 4096,
            },
            identity: RawIdentity {
                hostname: "win-server".to_string(),
                os_version: "10.0.22631".to_string(),
                architecture: "x86_64".to_string(),
                logical_cores: 8,
                physical_memory_bytes: 16_000_000_000,
                processor_group_count: 1,
            },
            cpu_error: false,
            topology_error: false,
            memory_error: false,
            commit_error: false,
            identity_error: false,
            drives_error: false,
            auto_increment_cpu: false,
            cpu_call_count: std::sync::atomic::AtomicU32::new(0),
            cpu_frequency: None,
            disk: Vec::new(),
            network: Vec::new(),
        }
    }
}

impl WindowsSource for MockWindowsSource {
    fn logical_drives(&self) -> Result<Vec<RawLogicalDrive>, CollectError> {
        if self.drives_error {
            return Err(CollectError::new(
                crate::error::CollectErrorKind::SourceUnavailable,
                "mock logical drive error",
            ));
        }
        Ok(self.drives.clone())
    }

    fn cpu_times(&self) -> Result<RawCpuTimes, CollectError> {
        if self.cpu_error {
            return Err(CollectError::new(
                crate::error::CollectErrorKind::SourceUnavailable,
                "mock cpu error",
            ));
        }
        if self.auto_increment_cpu {
            use std::sync::atomic::Ordering;
            let call = self.cpu_call_count.fetch_add(1, Ordering::Relaxed);
            // Each call after the first adds ticks to produce a valid delta.
            let offset = u64::from(call) * 100;
            return Ok(RawCpuTimes {
                idle: self.cpu.idle + offset,
                kernel: self.cpu.kernel + offset,
                user: self.cpu.user + offset,
            });
        }
        Ok(self.cpu)
    }

    fn active_processor_count(&self) -> Result<u32, CollectError> {
        if self.topology_error {
            return Err(CollectError::new(
                crate::error::CollectErrorKind::SourceUnavailable,
                "mock topology error",
            ));
        }
        Ok(self.topology.active_logical_processors)
    }

    fn processor_topology(&self) -> Result<RawProcessorTopology, CollectError> {
        if self.topology_error {
            return Err(CollectError::new(
                crate::error::CollectErrorKind::SourceUnavailable,
                "mock topology error",
            ));
        }
        Ok(self.topology)
    }

    fn physical_memory(&self) -> Result<RawPhysicalMemory, CollectError> {
        if self.memory_error {
            return Err(CollectError::new(
                crate::error::CollectErrorKind::SourceUnavailable,
                "mock memory error",
            ));
        }
        Ok(self.memory)
    }

    fn commit(&self) -> Result<RawCommit, CollectError> {
        if self.commit_error {
            return Err(CollectError::new(
                crate::error::CollectErrorKind::SourceUnavailable,
                "mock commit error",
            ));
        }
        Ok(self.commit)
    }

    fn identity(&self) -> Result<RawIdentity, CollectError> {
        if self.identity_error {
            return Err(CollectError::new(
                crate::error::CollectErrorKind::SourceUnavailable,
                "mock identity error",
            ));
        }
        Ok(self.identity.clone())
    }

    fn cpu_frequency_hz(&self) -> Result<Option<u64>, CollectError> {
        Ok(self.cpu_frequency)
    }
    fn disk_io(&self) -> Result<Vec<RawDiskIo>, CollectError> {
        Ok(self.disk.clone())
    }
    fn network_interfaces(&self) -> Result<Vec<RawNetworkInterface>, CollectError> {
        Ok(self.network.clone())
    }
}

// ---------------------------------------------------------------------------
// Production FFI implementation
// ---------------------------------------------------------------------------

/// Production implementation backed by Windows system APIs.
///
/// # Safety
///
/// All unsafe blocks in this implementation:
/// - Initialize structures with correct `cb`/`dwLength` sizes before API calls.
/// - Check return values and `GetLastError()` before reading output buffers.
/// - Copy owned data out of temporary buffers before returning.
/// - Never expose raw pointers or borrowed foreign memory across the boundary.
#[derive(Debug, Clone, Copy)]
pub struct NativeWindowsSource;

impl WindowsSource for NativeWindowsSource {
    fn logical_drives(&self) -> Result<Vec<RawLogicalDrive>, CollectError> {
        logical_drives()
    }

    fn cpu_times(&self) -> Result<RawCpuTimes, CollectError> {
        cpu_times()
    }

    fn active_processor_count(&self) -> Result<u32, CollectError> {
        active_processor_count()
    }

    fn processor_topology(&self) -> Result<RawProcessorTopology, CollectError> {
        processor_topology()
    }

    fn physical_memory(&self) -> Result<RawPhysicalMemory, CollectError> {
        physical_memory()
    }

    fn commit(&self) -> Result<RawCommit, CollectError> {
        commit()
    }

    fn identity(&self) -> Result<RawIdentity, CollectError> {
        collect_raw_identity()
    }

    fn cpu_frequency_hz(&self) -> Result<Option<u64>, CollectError> {
        cpu_frequency_hz()
    }

    fn disk_io(&self) -> Result<Vec<RawDiskIo>, CollectError> {
        disk_io()
    }

    fn network_interfaces(&self) -> Result<Vec<RawNetworkInterface>, CollectError> {
        network_interfaces()
    }
}

// ---------------------------------------------------------------------------
// Windows FFI helpers (native only)
//
// Raw extern declarations for Windows system APIs. This avoids depending
// on specific windows-sys feature flags and keeps the FFI surface minimal.
// All unsafe blocks document their safety invariants.
// ---------------------------------------------------------------------------

/// `ERROR_INSUFFICIENT_BUFFER` from Win32.
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// `ALL_PROCESSOR_GROUPS` constant from Win32.
const ALL_PROCESSOR_GROUPS: u16 = 0x0000_FFFF;

/// `ComputerNameDnsHostname` from `COMPUTER_NAME_FORMAT` enum.
const COMPUTER_NAME_DNS_HOSTNAME: u32 = 3;
pub(crate) const DRIVE_FIXED: u32 = 3;
pub(crate) const DRIVE_REMOVABLE: u32 = 2;

// ---------------------------------------------------------------------------
// Win32 control-code composition (winioctl.h / winnt.h; stable ABI)
// ---------------------------------------------------------------------------

/// Compose a device control code.
///
/// `CTL_CODE(DeviceType, Function, Method, Access)` is
/// `DeviceType << 16 | Access << 14 | Function << 2 | Method`, with `Method`
/// occupying the low two bits. Composing the published constants keeps a
/// transcription error out of the IOCTL table below.
const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

/// `FILE_DEVICE_DISK` from `winioctl.h`.
const FILE_DEVICE_DISK: u32 = 0x0007;
/// `METHOD_BUFFERED` from `winioctl.h`.
const METHOD_BUFFERED: u32 = 0x0000;
/// `FILE_READ_ACCESS` from `winnt.h`.
const FILE_READ_ACCESS: u32 = 0x0001;

/// `IOCTL_DISK_PERFORMANCE` — the `DISK_PERFORMANCE` counter query this
/// collector's output structure is shaped for.
///
/// It is `CTL_CODE(FILE_DEVICE_DISK, 0x20, METHOD_BUFFERED, FILE_READ_ACCESS)`.
/// The near-miss `0x0007_0020` decodes as `FILE_DEVICE_DISK`, function `0x08`,
/// `METHOD_BUFFERED`, `FILE_ANY_ACCESS` — that is
/// `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX`, a different request with a different
/// output structure, so it can never populate these counters.
const IOCTL_DISK_PERFORMANCE: u32 =
    ctl_code(FILE_DEVICE_DISK, 0x20, METHOD_BUFFERED, FILE_READ_ACCESS);

/// Access granted to a physical-drive handle.
///
/// The I/O manager compares the IOCTL's `RequiresFileAccess` against the
/// access already granted to the handle *before* the driver sees the request,
/// so a zero-access handle is refused with `ERROR_ACCESS_DENIED` and
/// `CreateFileW` returns `INVALID_HANDLE_VALUE`. It must therefore carry at
/// least the `FILE_READ_ACCESS` that `IOCTL_DISK_PERFORMANCE` declares.
const DISK_HANDLE_ACCESS: u32 = FILE_READ_ACCESS;

// ---------------------------------------------------------------------------
// Network interface types (`IF_TYPE` from `ipifcons.h`; stable ABI)
// ---------------------------------------------------------------------------

/// `IF_TYPE_PPP` — point-to-point protocol link.
const IF_TYPE_PPP: u32 = 23;
/// `IF_TYPE_SOFTWARE_LOOPBACK` — the software loopback adapter.
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
/// `IF_TYPE_TUNNEL` — encapsulation/tunnel interface.
const IF_TYPE_TUNNEL: u32 = 131;

/// Whether an `MIB_IF_ROW2.Type` row may contribute to the network aggregate.
///
/// A tunnel row counts the same bytes twice whenever its underlay link is also
/// aggregated: the tunnel counters hold the encapsulated frames and the
/// physical NIC counts the identical frames again. Because the directional
/// capacities are summed from the same rows, the overstatement inflates the
/// utilization denominator as well as the numerator. Linux already excludes
/// this class through a slave interface's `master` symlink; Windows exposes it
/// as the interface type.
///
/// Physical Ethernet (`IF_TYPE_ETHERNET_CSMACD`, 6) and Wi-Fi
/// (`IF_TYPE_IEEE80211`, 71) stay members — they are the underlay the
/// double-count was hiding behind.
const fn is_aggregate_member_type(interface_type: u32) -> bool {
    !matches!(
        interface_type,
        IF_TYPE_PPP | IF_TYPE_SOFTWARE_LOOPBACK | IF_TYPE_TUNNEL
    )
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod ffi {
    //! Raw Windows FFI declarations for system information APIs.
    //!
    //! # Safety
    //!
    //! Every extern function is documented in Microsoft's Win32 API reference.
    //! Callers must ensure:
    //! - Structure `cbSize`/`dwLength` fields are initialized before the call.
    //! - Output buffers are valid for the declared length.
    //! - Return values are checked before reading output.

    /// `FILETIME` — 100-nanosecond intervals since January 1, 1601.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct FileTime {
        pub dw_low_date_time: u32,
        pub dw_high_date_time: u32,
    }

    /// `MEMORYSTATUSEX` — extended memory status.
    #[repr(C)]
    pub struct MemoryStatusEx {
        pub dw_length: u32,
        pub memory_load: u32,
        pub ull_total_phys: u64,
        pub ull_avail_phys: u64,
        pub ull_total_page_file: u64,
        pub ull_avail_page_file: u64,
        pub ull_total_virtual: u64,
        pub ull_avail_virtual: u64,
        pub ull_avail_extended_virtual: u64,
    }

    /// `PERFORMANCE_INFORMATION` — system performance counters.
    ///
    /// Member types are ABI-faithful, not "all the same integer". `cb`,
    /// `handles_count`, `process_count` and `thread_count` are `DWORD`; every
    /// other member is `SIZE_T`. Declaring them all `usize` both misreports the
    /// buffer size the API validates `cb` against and moves those four counters
    /// by four bytes each, so the layout is asserted below rather than assumed.
    /// The members are kept in the Win32 header's order, which is also the
    /// offset order: `physical_total` precedes `physical_available`.
    #[repr(C)]
    pub struct PerformanceInformation {
        pub cb: u32,
        pub commit_total: usize,
        pub commit_limit: usize,
        pub commit_peak: usize,
        pub physical_total: usize,
        pub physical_available: usize,
        pub system_cache: usize,
        pub kernel_total: usize,
        pub kernel_paged: usize,
        pub kernel_nonpaged: usize,
        pub page_size: usize,
        pub handles_count: u32,
        pub process_count: u32,
        pub thread_count: u32,
    }

    /// The mirror is used as a fixed-size in/out buffer, so its size and the
    /// offsets of the members this crate reads are pinned rather than derived:
    /// a `DWORD` widened to `usize` would still compile, still link, and silently
    /// report commit charge from the wrong fields.
    const _: () = {
        assert!(std::mem::size_of::<PerformanceInformation>() == 104);
        assert!(std::mem::offset_of!(PerformanceInformation, cb) == 0);
        assert!(std::mem::offset_of!(PerformanceInformation, commit_total) == 8);
        assert!(std::mem::offset_of!(PerformanceInformation, commit_limit) == 16);
        assert!(std::mem::offset_of!(PerformanceInformation, commit_peak) == 24);
        assert!(std::mem::offset_of!(PerformanceInformation, physical_total) == 32);
        assert!(
            std::mem::offset_of!(PerformanceInformation, physical_available) == 40,
            "PhysicalTotal precedes PhysicalAvailable"
        );
        assert!(std::mem::offset_of!(PerformanceInformation, system_cache) == 48);
        assert!(std::mem::offset_of!(PerformanceInformation, kernel_total) == 56);
        assert!(std::mem::offset_of!(PerformanceInformation, kernel_paged) == 64);
        assert!(std::mem::offset_of!(PerformanceInformation, kernel_nonpaged) == 72);
        assert!(std::mem::offset_of!(PerformanceInformation, page_size) == 80);
        assert!(std::mem::offset_of!(PerformanceInformation, handles_count) == 88);
        assert!(std::mem::offset_of!(PerformanceInformation, process_count) == 92);
        assert!(std::mem::offset_of!(PerformanceInformation, thread_count) == 96);
    };

    /// `OSVERSIONINFOW` — OS version information.
    #[repr(C)]
    pub struct OsVersionInfoW {
        pub dw_os_version_info_size: u32,
        pub dw_major_version: u32,
        pub dw_minor_version: u32,
        pub dw_build_number: u32,
        pub dw_platform_id: u32,
        pub sz_c_version_string: [u16; 128],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct ProcessorPowerInformation {
        pub number: u32,
        pub max_mhz: u32,
        pub current_mhz: u32,
        pub mhz_limit: u32,
        pub max_idle_state: u32,
        pub current_idle_state: u32,
    }

    #[repr(C)]
    pub struct DiskPerformance {
        pub bytes_read: u64,
        pub bytes_written: u64,
        pub read_time: i64,
        pub write_time: i64,
        pub idle_time: i64,
        pub read_count: u32,
        pub write_count: u32,
        pub queue_depth: u32,
        pub split_count: u32,
        pub query_time: i64,
        pub storage_device_number: u32,
        pub storage_manager_name: [u16; 8],
    }

    /// `MIB_IF_ROW2` — one interface row of `GetIfTable2`.
    ///
    /// The six `*Octets` members the SDK places inline are declared even
    /// though this collector only reads four of them: every omitted member
    /// shifts the ones after it, so `OutOctets` would read `InUcastOctets`
    /// and every row after the first would be stepped by a wrong stride.
    /// The layout is pinned by the assertions below, which is the only thing
    /// a `#[repr(C)]` mirror of an SDK struct can be checked against.
    #[repr(C)]
    pub struct MibIfRow2 {
        pub interface_luid: u64,
        pub interface_index: u32,
        pub interface_guid: [u8; 16],
        pub alias: [u16; 257],
        pub description: [u16; 257],
        pub physical_address_length: u32,
        pub physical_address: [u8; 32],
        pub permanent_physical_address: [u8; 32],
        pub mtu: u32,
        pub interface_type: u32,
        pub tunnel_type: u32,
        pub media_type: u32,
        pub physical_medium_type: u32,
        pub access_type: u32,
        pub direction_type: u32,
        pub interface_and_oper_status_flags: u8,
        pub oper_status: u32,
        pub admin_status: u32,
        pub media_connect_state: u32,
        pub network_guid: [u8; 16],
        pub connection_type: u32,
        pub transmit_link_speed: u64,
        pub receive_link_speed: u64,
        pub in_octets: u64,
        pub in_ucast_pkts: u64,
        pub in_nucast_pkts: u64,
        pub in_discards: u64,
        pub in_errors: u64,
        pub in_unknown_protos: u64,
        pub in_ucast_octets: u64,
        pub in_multicast_octets: u64,
        pub in_broadcast_octets: u64,
        pub out_octets: u64,
        pub out_ucast_pkts: u64,
        pub out_nucast_pkts: u64,
        pub out_discards: u64,
        pub out_errors: u64,
        pub out_ucast_octets: u64,
        pub out_multicast_octets: u64,
        pub out_broadcast_octets: u64,
        pub out_qlen: u64,
    }

    /// The real `MIB_IF_ROW2` is 1352 bytes on `x86_64` with every member
    /// declared; a short mirror also strides `from_raw_parts` through the API's
    /// own allocation incorrectly, so the size is asserted rather than assumed.
    const _: () = assert!(std::mem::size_of::<MibIfRow2>() == 1352);
    /// The members this collector reads are asserted individually so a wrong
    /// mirror cannot silently line them up with a neighbouring counter.
    const _: () = assert!(std::mem::offset_of!(MibIfRow2, in_octets) == 1208);
    const _: () = assert!(std::mem::offset_of!(MibIfRow2, out_octets) == 1280);
    const _: () = assert!(std::mem::offset_of!(MibIfRow2, out_qlen) == 1344);
    const _: () = assert!(std::mem::offset_of!(MibIfRow2, interface_type) == 1128);

    /// `SYSTEM_INFO` — processor architecture info (simplified for `x86_64`).
    #[repr(C)]
    #[cfg(target_arch = "x86_64")]
    pub struct SystemInfo {
        pub w_processor_architecture: u16,
        pub w_reserved: u16,
        pub dw_page_size: u32,
        pub lp_min_application_address: *mut u8,
        pub lp_max_application_address: *mut u8,
        pub dw_active_processor_mask: usize,
        pub dw_number_of_processors: u32,
        pub dw_processor_type: u32,
        pub dw_allocation_granularity: u32,
        pub w_processor_level: u16,
        pub w_processor_revision: u16,
    }

    #[link(name = "kernel32")]
    #[link(name = "ntdll")]
    #[link(name = "psapi")]
    #[link(name = "powrprof")]
    #[link(name = "iphlpapi")]
    extern "system" {
        pub fn GetSystemTimes(
            idle_time: *mut FileTime,
            kernel_time: *mut FileTime,
            user_time: *mut FileTime,
        ) -> i32;

        pub fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;

        pub fn GetPerformanceInfo(
            p_performance_information: *mut PerformanceInformation,
            cb: u32,
        ) -> i32;

        pub fn GetActiveProcessorCount(group_number: u16) -> u32;

        pub fn GetActiveProcessorGroupCount() -> u16;

        pub fn GetComputerNameExW(name_type: u32, buffer: *mut u16, size: *mut u32) -> i32;

        pub fn RtlGetVersion(lp_version_information: *mut OsVersionInfoW) -> i32;

        pub fn GetSystemInfo(lp_system_info: *mut SystemInfo);

        pub fn GetLastError() -> u32;

        pub fn GetLogicalDriveStringsW(buffer_length: u32, buffer: *mut u16) -> u32;
        pub fn GetDriveTypeW(root_path_name: *const u16) -> u32;
        pub fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            free_bytes_available: *mut u64,
            total_number_of_bytes: *mut u64,
            total_number_of_free_bytes: *mut u64,
        ) -> i32;

        pub fn CallNtPowerInformation(
            information_level: u32,
            input_buffer: *const u8,
            input_buffer_length: u32,
            output_buffer: *mut u8,
            output_buffer_length: u32,
        ) -> i32;
        pub fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const u8,
            creation: u32,
            flags: u32,
            template: *const u8,
        ) -> *mut std::ffi::c_void;
        pub fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        pub fn DeviceIoControl(
            handle: *mut std::ffi::c_void,
            code: u32,
            input: *const u8,
            input_size: u32,
            output: *mut u8,
            output_size: u32,
            returned: *mut u32,
            overlapped: *mut u8,
        ) -> i32;
        pub fn GetIfTable2(table: *mut *mut MibIfRow2) -> u32;
        pub fn FreeMibTable(table: *mut MibIfRow2);
    }
}

/// Convert a `FileTime` (100-nanosecond intervals since 1601) to a `u64`.
///
/// # Safety
///
/// This is a plain bit-pattern read: the two `u32` halves of `ft` are
/// recombined into a single `u64`, so every bit pattern yields a valid
/// value and no memory unsafety can occur for any input. The result is
/// only a meaningful filetime when `ft` was populated by a Windows API
/// call; an arbitrary or uninitialized value produces an arbitrary
/// timestamp, not undefined behavior.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
unsafe fn filetime_to_u64(ft: ffi::FileTime) -> u64 {
    u64::from(ft.dw_low_date_time) | (u64::from(ft.dw_high_date_time) << 32)
}

fn logical_drives() -> Result<Vec<RawLogicalDrive>, CollectError> {
    #[cfg(target_os = "windows")]
    {
        let mut buffer = vec![0_u16; 256];
        let mut required = validate_drive_string_count(unsafe {
            // Safety: buffer is writable and its declared length matches its
            // allocation. The returned size is checked before parsing.
            ffi::GetLogicalDriveStringsW(
                u32::try_from(buffer.len()).map_err(|_| {
                    CollectError::new(
                        CollectErrorKind::Numeric,
                        "logical-drive buffer size exceeds API limit",
                    )
                })?,
                buffer.as_mut_ptr(),
            )
        })?;
        if required >= buffer.len() {
            let size = required.checked_add(1).ok_or_else(|| {
                CollectError::new(
                    CollectErrorKind::Numeric,
                    "logical-drive buffer size overflow",
                )
            })?;
            buffer.resize(size, 0);
            let retry_required = unsafe {
                // Safety: the resized buffer is writable and the API receives
                // its exact capacity.
                ffi::GetLogicalDriveStringsW(
                    u32::try_from(buffer.len()).map_err(|_| {
                        CollectError::new(
                            CollectErrorKind::Numeric,
                            "logical-drive buffer size exceeds API limit",
                        )
                    })?,
                    buffer.as_mut_ptr(),
                )
            };
            required = validate_drive_string_count(retry_required)?;
            if required >= buffer.len() {
                return Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "GetLogicalDriveStringsW buffer remained insufficient",
                ));
            }
        }

        let mut result = Vec::new();
        for root in buffer[..required].split(|value| *value == 0) {
            if root.is_empty() {
                continue;
            }
            let root = String::from_utf16(root).map_err(|_| {
                CollectError::new(
                    CollectErrorKind::Parse,
                    "logical drive root is invalid UTF-16",
                )
            })?;
            let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
            let drive_type = unsafe {
                // Safety: wide is NUL-terminated and lives through this call.
                ffi::GetDriveTypeW(wide.as_ptr())
            };
            if drive_type != DRIVE_FIXED && drive_type != DRIVE_REMOVABLE {
                continue;
            }
            let mut available = 0_u64;
            let mut total = 0_u64;
            let mut free = 0_u64;
            let success = unsafe {
                // Safety: all output pointers reference initialized writable
                // locals and the root pointer is valid for this call.
                ffi::GetDiskFreeSpaceExW(
                    wide.as_ptr(),
                    std::ptr::from_mut(&mut available),
                    std::ptr::from_mut(&mut total),
                    std::ptr::from_mut(&mut free),
                )
            };
            if success == 0 || total == 0 || free > total || available > total {
                continue;
            }
            result.push(RawLogicalDrive {
                root,
                drive_type,
                total_bytes: total,
                total_free_bytes: free,
                available_bytes: available,
            });
        }
        // Windows volume roots are case-insensitive, so `C:\` and `c:\` name
        // the same volume. The sort uses the same case-insensitive key the
        // de-duplication compares, because `dedup_by` only ever compares
        // adjacent entries: an ordinal sort leaves `C:\`, `D:\`, `c:\` and the
        // duplicate survives.
        result.sort_by(|left, right| {
            left.root
                .to_ascii_lowercase()
                .cmp(&right.root.to_ascii_lowercase())
        });
        result.dedup_by(|left, right| left.root.eq_ignore_ascii_case(&right.root));
        Ok(result)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "Windows drive APIs not available on this platform",
        ))
    }
}

#[cfg(target_os = "windows")]
fn validate_drive_string_count(required: u32) -> Result<usize, CollectError> {
    if required == 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "GetLogicalDriveStringsW returned zero",
        ));
    }
    Ok(required as usize)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{
        ctl_code, is_aggregate_member_type, validate_drive_string_count, DISK_HANDLE_ACCESS,
        FILE_DEVICE_DISK, FILE_READ_ACCESS, IF_TYPE_PPP, IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL,
        IOCTL_DISK_PERFORMANCE, METHOD_BUFFERED,
    };
    use crate::error::CollectErrorKind;

    #[test]
    fn zero_initial_length_is_source_failure() {
        let error = validate_drive_string_count(0).expect_err("zero initial result must fail");
        assert_eq!(error.kind, CollectErrorKind::SourceUnavailable);
    }

    #[test]
    fn zero_retry_length_is_source_failure() {
        let error = validate_drive_string_count(0).expect_err("zero retry result must fail");
        assert_eq!(error.kind, CollectErrorKind::SourceUnavailable);
    }

    /// `IOCTL_DISK_PERFORMANCE` recomputed from its four `CTL_CODE` parts.
    ///
    /// The literal is pinned against the published Win32 value and against the
    /// composition itself, so a future edit cannot reintroduce a code that
    /// decodes to some other request.
    #[test]
    fn disk_performance_ioctl_matches_ctl_code_composition() {
        assert_eq!(IOCTL_DISK_PERFORMANCE, 0x0007_4080);
        assert_eq!(
            IOCTL_DISK_PERFORMANCE,
            ctl_code(FILE_DEVICE_DISK, 0x20, METHOD_BUFFERED, FILE_READ_ACCESS)
        );
        // Device type, function, method and access decode back out of the
        // published literal exactly as `CTL_CODE` laid them down.
        assert_eq!(IOCTL_DISK_PERFORMANCE >> 16, FILE_DEVICE_DISK);
        assert_eq!((IOCTL_DISK_PERFORMANCE >> 14) & 0x3, FILE_READ_ACCESS);
        assert_eq!((IOCTL_DISK_PERFORMANCE >> 2) & 0xFFF, 0x20);
        assert_eq!(IOCTL_DISK_PERFORMANCE & 0x3, METHOD_BUFFERED);
    }

    /// The code this collector used to send is a *different* request.
    ///
    /// `0x0007_0020` decodes as `FILE_DEVICE_DISK` function `0x08` with
    /// `FILE_ANY_ACCESS` (`0`) — `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX` — which
    /// returns a geometry structure, never `DISK_PERFORMANCE` counters.
    #[test]
    fn geometry_ex_literal_is_not_the_performance_ioctl() {
        const FILE_ANY_ACCESS: u32 = 0;
        let geometry_ex = ctl_code(FILE_DEVICE_DISK, 0x08, METHOD_BUFFERED, FILE_ANY_ACCESS);
        assert_eq!(geometry_ex, 0x0007_0020);
        assert_ne!(geometry_ex, IOCTL_DISK_PERFORMANCE);
        // The cross-checked `IOCTL_DISK_GET_LENGTH_INFO` value composes
        // through the same formula and must differ from both.
        assert_eq!(
            ctl_code(FILE_DEVICE_DISK, 0x17, METHOD_BUFFERED, FILE_READ_ACCESS),
            0x0007_405C
        );
    }

    /// The handle access must satisfy the IOCTL's own access requirement.
    ///
    /// `DeviceIoControl`'s `RequiresFileAccess` is checked against the access
    /// already granted to the handle before the driver runs, so a handle opened
    /// without `FILE_READ_ACCESS` never reaches `DISK_PERFORMANCE` at all.
    #[test]
    fn disk_handle_access_satisfies_the_ioctl_requirement() {
        assert_eq!(DISK_HANDLE_ACCESS, FILE_READ_ACCESS);
        assert_eq!(FILE_READ_ACCESS, 1);
        assert_eq!((IOCTL_DISK_PERFORMANCE >> 14) & 0x3, DISK_HANDLE_ACCESS);
        assert_ne!(
            DISK_HANDLE_ACCESS, 0,
            "a zero-access handle fails with ERROR_ACCESS_DENIED"
        );
    }

    /// Tunnel, PPP and loopback rows leave the aggregate; physical links stay.
    #[test]
    fn aggregate_member_excludes_tunnel_ppp_and_loopback_types() {
        assert!(!is_aggregate_member_type(IF_TYPE_SOFTWARE_LOOPBACK));
        assert!(!is_aggregate_member_type(IF_TYPE_TUNNEL));
        assert!(!is_aggregate_member_type(IF_TYPE_PPP));
        // `IF_TYPE_ETHERNET_CSMACD` and `IF_TYPE_IEEE80211` are the physical
        // underlay the tunnel double-count was hiding behind.
        assert!(is_aggregate_member_type(6));
        assert!(is_aggregate_member_type(71));
        // An unrecognised type stays a member: only the tunnel family is
        // excluded, never physical Ethernet or Wi-Fi.
        assert!(is_aggregate_member_type(117));
    }
}

/// Read cumulative CPU time counters from `GetSystemTimes`.
///
/// Returns 100-nanosecond interval counts for idle, kernel (including
/// idle), and user time.
fn cpu_times() -> Result<RawCpuTimes, CollectError> {
    #[cfg(target_os = "windows")]
    {
        use std::mem::MaybeUninit;

        unsafe {
            let mut idle = MaybeUninit::<ffi::FileTime>::uninit();
            let mut kernel = MaybeUninit::<ffi::FileTime>::uninit();
            let mut user = MaybeUninit::<ffi::FileTime>::uninit();

            let success =
                ffi::GetSystemTimes(idle.as_mut_ptr(), kernel.as_mut_ptr(), user.as_mut_ptr());

            if success == 0 {
                return Err(CollectError::new(
                    crate::error::CollectErrorKind::SourceUnavailable,
                    "GetSystemTimes failed",
                ));
            }

            let idle_ft = idle.assume_init();
            let kernel_ft = kernel.assume_init();
            let user_ft = user.assume_init();

            Ok(RawCpuTimes {
                idle: filetime_to_u64(idle_ft),
                kernel: filetime_to_u64(kernel_ft),
                user: filetime_to_u64(user_ft),
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::error::CollectErrorKind::SourceUnavailable,
            "Windows CPU APIs not available on this platform",
        ))
    }
}

/// Read the total number of active logical processors across all groups.
fn active_processor_count() -> Result<u32, CollectError> {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            let count = ffi::GetActiveProcessorCount(ALL_PROCESSOR_GROUPS);
            if count == 0 {
                return Err(CollectError::new(
                    crate::error::CollectErrorKind::SourceUnavailable,
                    "GetActiveProcessorCount failed",
                ));
            }
            Ok(count)
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::error::CollectErrorKind::SourceUnavailable,
            "Windows processor APIs not available on this platform",
        ))
    }
}

/// Read processor topology (group count and total logical processors).
fn processor_topology() -> Result<RawProcessorTopology, CollectError> {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            let group_count = ffi::GetActiveProcessorGroupCount();
            let total = active_processor_count()?;
            Ok(RawProcessorTopology {
                active_logical_processors: total,
                group_count,
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::error::CollectErrorKind::SourceUnavailable,
            "Windows processor APIs not available on this platform",
        ))
    }
}

/// Read physical memory statistics from `GlobalMemoryStatusEx`.
fn physical_memory() -> Result<RawPhysicalMemory, CollectError> {
    #[cfg(target_os = "windows")]
    {
        use std::mem::MaybeUninit;

        unsafe {
            let mut mem_info = MaybeUninit::<ffi::MemoryStatusEx>::uninit();
            #[allow(clippy::cast_possible_truncation)]
            {
                (*mem_info.as_mut_ptr()).dw_length =
                    std::mem::size_of::<ffi::MemoryStatusEx>() as u32;
            }

            let success = ffi::GlobalMemoryStatusEx(mem_info.as_mut_ptr());

            if success == 0 {
                return Err(CollectError::new(
                    crate::error::CollectErrorKind::SourceUnavailable,
                    "GlobalMemoryStatusEx failed",
                ));
            }

            let info = mem_info.assume_init();
            Ok(RawPhysicalMemory {
                total_bytes: info.ull_total_phys,
                available_bytes: info.ull_avail_phys,
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::error::CollectErrorKind::SourceUnavailable,
            "Windows memory APIs not available on this platform",
        ))
    }
}

/// Read commit charge from `GetPerformanceInfo`.
fn commit() -> Result<RawCommit, CollectError> {
    #[cfg(target_os = "windows")]
    {
        use std::mem::MaybeUninit;

        // The buffer is zeroed rather than left uninitialised: the API writes at
        // most `cb` bytes and the layout assertions above pin that to this
        // struct's size, so every member read below was written by the call and
        // nothing is read as undefined memory.
        let cb =
            u32::try_from(std::mem::size_of::<ffi::PerformanceInformation>()).unwrap_or(u32::MAX);
        // SAFETY: `perf_info` is a live, fully initialised, correctly aligned
        // buffer of exactly `cb` bytes for the whole call, and `GetPerformanceInfo`
        // writes no more than `cb` bytes into it. `cb` is the first member, as
        // the API requires. Every field read after `assume_init` was therefore
        // written by the call, since `success != 0` means the write happened.
        unsafe {
            let mut perf_info = MaybeUninit::<ffi::PerformanceInformation>::zeroed();
            (*perf_info.as_mut_ptr()).cb = cb;
            let success = ffi::GetPerformanceInfo(perf_info.as_mut_ptr(), cb);

            if success == 0 {
                let err = ffi::GetLastError();
                if err == ERROR_INSUFFICIENT_BUFFER {
                    return Err(CollectError::new(
                        crate::error::CollectErrorKind::SourceUnavailable,
                        "GetPerformanceInfo requires larger buffer",
                    ));
                }
                return Err(CollectError::new(
                    crate::error::CollectErrorKind::SourceUnavailable,
                    "GetPerformanceInfo failed",
                ));
            }

            let info = perf_info.assume_init();
            Ok(RawCommit {
                commit_total_pages: u64::try_from(info.commit_total).unwrap_or(u64::MAX),
                commit_limit_pages: u64::try_from(info.commit_limit).unwrap_or(u64::MAX),
                page_size_bytes: u64::try_from(info.page_size).unwrap_or(1).max(1),
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::error::CollectErrorKind::SourceUnavailable,
            "Windows performance APIs not available on this platform",
        ))
    }
}

/// Query current processor frequency using the documented native power API.
fn cpu_frequency_hz() -> Result<Option<u64>, CollectError> {
    #[cfg(target_os = "windows")]
    {
        let count = active_processor_count()?;
        let count = usize::try_from(count).map_err(|_| {
            CollectError::new(CollectErrorKind::Numeric, "processor count overflow")
        })?;
        if count == 0 || count > 4096 {
            return Ok(None);
        }
        let size = count
            .checked_mul(std::mem::size_of::<ffi::ProcessorPowerInformation>())
            .ok_or_else(|| {
                CollectError::new(
                    CollectErrorKind::Numeric,
                    "processor information size overflow",
                )
            })?;
        let mut buffer = vec![ffi::ProcessorPowerInformation::default(); count];
        let status = unsafe {
            // Safety: buffer is writable and sized for one record per active
            // processor; the API only writes the declared output length.
            ffi::CallNtPowerInformation(
                11,
                std::ptr::null(),
                0,
                buffer.as_mut_ptr().cast(),
                u32::try_from(size).map_err(|_| {
                    CollectError::new(
                        CollectErrorKind::Numeric,
                        "processor information size exceeds API limit",
                    )
                })?,
            )
        };
        if status != 0 {
            return Ok(None);
        }
        let records = &buffer;
        let mut sum = 0u64;
        let mut valid = 0u64;
        for record in records {
            if record.current_mhz == 0 {
                continue;
            }
            sum = sum
                .checked_add(u64::from(record.current_mhz))
                .ok_or_else(|| {
                    CollectError::new(
                        CollectErrorKind::Numeric,
                        "processor frequency sum overflow",
                    )
                })?;
            valid += 1;
        }
        let Some(mean_mhz) = (valid > 0).then_some(sum / valid) else {
            return Ok(None);
        };
        Ok(mean_mhz.checked_mul(1_000))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(None)
    }
}

/// Query per-disk byte counters with bounded direct handles. Failed disks are
/// skipped because this is optional telemetry.
#[allow(clippy::unnecessary_wraps)]
fn disk_io() -> Result<Vec<RawDiskIo>, CollectError> {
    #[cfg(target_os = "windows")]
    {
        let mut records = Vec::new();
        for index in 0..32u32 {
            let path: Vec<u16> = format!(r"\\.\PhysicalDrive{index}")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let handle = unsafe {
                ffi::CreateFileW(
                    path.as_ptr(),
                    DISK_HANDLE_ACCESS,
                    3,
                    std::ptr::null(),
                    3,
                    0,
                    std::ptr::null(),
                )
            };
            if handle as isize == -1 {
                continue;
            }
            let mut performance = std::mem::MaybeUninit::<ffi::DiskPerformance>::zeroed();
            let mut returned = 0u32;
            let ok = unsafe {
                // Safety: the handle is valid for this call and the output
                // buffer has the declared DISK_PERFORMANCE size.
                ffi::DeviceIoControl(
                    handle,
                    IOCTL_DISK_PERFORMANCE,
                    std::ptr::null(),
                    0,
                    performance.as_mut_ptr().cast(),
                    u32::try_from(std::mem::size_of::<ffi::DiskPerformance>()).map_err(|_| {
                        CollectError::new(
                            CollectErrorKind::Numeric,
                            "disk performance structure size exceeds API limit",
                        )
                    })?,
                    std::ptr::from_mut(&mut returned),
                    std::ptr::null_mut(),
                )
            } != 0;
            if ok
                && returned
                    >= u32::try_from(std::mem::size_of::<ffi::DiskPerformance>()).map_err(|_| {
                        CollectError::new(
                            CollectErrorKind::Numeric,
                            "disk performance structure size exceeds API limit",
                        )
                    })?
            {
                let value = unsafe { performance.assume_init() };
                records.push(RawDiskIo {
                    id: format!("physical-{index}"),
                    name: format!("PhysicalDrive{index}"),
                    read_bytes: value.bytes_read,
                    write_bytes: value.bytes_written,
                });
            }
            unsafe {
                ffi::CloseHandle(handle);
            }
        }
        Ok(records)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "Windows disk API unavailable",
        ))
    }
}

/// Enumerate interface rows with the documented IP Helper table API.
fn network_interfaces() -> Result<Vec<RawNetworkInterface>, CollectError> {
    #[cfg(target_os = "windows")]
    {
        let mut table = std::ptr::null_mut();
        let status = unsafe { ffi::GetIfTable2(std::ptr::from_mut(&mut table)) };
        if status != 0 || table.is_null() {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "GetIfTable2 failed",
            ));
        }
        let count = unsafe { (*table.cast::<u32>()) as usize };
        if count > 1024 {
            unsafe {
                ffi::FreeMibTable(table);
            }
            return Err(CollectError::new(
                CollectErrorKind::Numeric,
                "interface table exceeds bound",
            ));
        }
        #[allow(clippy::cast_ptr_alignment)]
        let first = unsafe {
            let table_offset =
                (std::mem::size_of::<u32>() + std::mem::align_of::<ffi::MibIfRow2>() - 1)
                    & !(std::mem::align_of::<ffi::MibIfRow2>() - 1);
            table
                .cast::<u8>()
                .add(table_offset)
                .cast::<ffi::MibIfRow2>()
        };
        let rows = unsafe { std::slice::from_raw_parts(first, count) };
        let mut records = Vec::with_capacity(count);
        for row in rows {
            let name = utf16_field(&row.alias);
            let is_loopback = row.interface_type == IF_TYPE_SOFTWARE_LOOPBACK;
            let operational = row.oper_status == 1 && row.media_connect_state == 1;
            records.push(RawNetworkInterface {
                id: format!("luid-{:016x}", row.interface_luid),
                name,
                rx_bytes: row.in_octets,
                tx_bytes: row.out_octets,
                rx_capacity_bps: (row.receive_link_speed > 0).then_some(row.receive_link_speed),
                tx_capacity_bps: (row.transmit_link_speed > 0).then_some(row.transmit_link_speed),
                is_loopback,
                operational,
                aggregate_member: is_aggregate_member_type(row.interface_type),
            });
        }
        unsafe {
            ffi::FreeMibTable(table);
        }
        records.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(records)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "Windows interface API unavailable",
        ))
    }
}

#[cfg(target_os = "windows")]
fn utf16_field(field: &[u16]) -> String {
    let end = field
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(field.len());
    String::from_utf16_lossy(&field[..end])
}

/// Collect raw identity fields via Windows APIs.
fn collect_raw_identity() -> Result<RawIdentity, CollectError> {
    let hostname = get_hostname()?;
    let (os_version, logical_cores) = get_os_info()?;
    let architecture = get_architecture();
    let physical_memory_bytes = physical_memory().map_or(0, |m| m.total_bytes);
    let topology = processor_topology().ok();

    Ok(RawIdentity {
        hostname,
        os_version,
        architecture,
        logical_cores,
        physical_memory_bytes,
        processor_group_count: topology.map_or(1, |t| t.group_count),
    })
}

/// Get the computer hostname via `GetComputerNameExW`.
fn get_hostname() -> Result<String, CollectError> {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            let mut size: u32 = 0;
            // First call to determine required buffer size.
            ffi::GetComputerNameExW(
                COMPUTER_NAME_DNS_HOSTNAME,
                std::ptr::null_mut(),
                std::ptr::from_mut(&mut size),
            );

            if size == 0 {
                return Err(CollectError::new(
                    crate::error::CollectErrorKind::SourceUnavailable,
                    "GetComputerNameExW returned zero size",
                ));
            }

            let mut buffer: Vec<u16> = vec![0; size as usize];
            let success = ffi::GetComputerNameExW(
                COMPUTER_NAME_DNS_HOSTNAME,
                buffer.as_mut_ptr(),
                std::ptr::from_mut(&mut size),
            );

            if success == 0 {
                return Err(CollectError::new(
                    crate::error::CollectErrorKind::SourceUnavailable,
                    "GetComputerNameExW failed",
                ));
            }

            // `size` is the number of UTF-16 code units written, not the
            // allocated buffer length.
            buffer.truncate(size as usize);

            String::from_utf16(&buffer).map_err(|_| {
                CollectError::new(
                    crate::error::CollectErrorKind::Parse,
                    "hostname contains invalid UTF-16",
                )
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::error::CollectErrorKind::SourceUnavailable,
            "Windows hostname API not available on this platform",
        ))
    }
}

/// Get OS version and logical core count via `RtlGetVersion` and
/// `GetSystemInfo`.
#[allow(clippy::unnecessary_wraps)]
fn get_os_info() -> Result<(String, u32), CollectError> {
    #[cfg(target_os = "windows")]
    {
        use std::mem::MaybeUninit;

        unsafe {
            // RtlGetVersion doesn't require manifest version lying.
            let mut version_ex = MaybeUninit::<ffi::OsVersionInfoW>::uninit();
            #[allow(clippy::cast_possible_truncation)]
            {
                (*version_ex.as_mut_ptr()).dw_os_version_info_size =
                    std::mem::size_of::<ffi::OsVersionInfoW>() as u32;
            }

            let status = ffi::RtlGetVersion(version_ex.as_mut_ptr());

            let version = if status >= 0 {
                let v = version_ex.assume_init();
                format!(
                    "{}.{}.{}",
                    v.dw_major_version, v.dw_minor_version, v.dw_build_number
                )
            } else {
                "unknown".to_string()
            };

            // Get logical processor count from system info.
            #[cfg(target_arch = "x86_64")]
            {
                let mut sys_info = MaybeUninit::<ffi::SystemInfo>::uninit();
                ffi::GetSystemInfo(sys_info.as_mut_ptr());
                let info = sys_info.assume_init();
                let logical_cores = info.dw_number_of_processors;
                Ok((version, logical_cores))
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                // Fallback for non-x86_64: use the active processor count.
                let logical_cores = active_processor_count().unwrap_or(1);
                Ok((version, logical_cores))
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::error::CollectErrorKind::SourceUnavailable,
            "Windows OS info APIs not available on this platform",
        ))
    }
}

/// Determine the target architecture string.
fn get_architecture() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        "x86_64".to_string()
    }
    #[cfg(target_arch = "aarch64")]
    {
        "aarch64".to_string()
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        std::env::consts::ARCH.to_string()
    }
}
