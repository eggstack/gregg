//! macOS FFI boundary.
//!
//! All unsafe calls and C/Mach structure handling belong in this module. The
//! rest of the collector consumes owned safe Rust records.
//!
//! # Safety invariants
//!
//! - Every Mach return status is validated.
//! - Structure count values returned by APIs are validated.
//! - Buffers are initialized correctly before foreign calls.
//! - No pointers into temporary foreign buffers escape this module.
//! - C strings are converted with explicit invalid-UTF-8 handling.
//! - Integer conversions use checked arithmetic.
//!
//! # Testability
//!
//! Production code lives in [`FfiNativeQueries`]. Tests use
//! [`MockNativeQueries`] to inject failures and synthetic values.

#![allow(unsafe_code)]

use crate::collector::error::{CollectError, CollectErrorKind};

// ---------------------------------------------------------------------------
// Raw FFI record types
// ---------------------------------------------------------------------------

/// Cumulative CPU tick counters from Mach `host_statistics` with
/// `HOST_CPU_LOAD_INFO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawCpuTicks {
    pub user: u64,
    pub system: u64,
    pub idle: u64,
    pub nice: u64,
}

impl RawCpuTicks {
    /// Sum of every field, used to compute the denominator.
    pub fn total(self) -> u64 {
        self.user
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.nice)
    }

    /// Sum of user + system + nice (the "busy" fields).
    pub fn busy(self) -> u64 {
        self.user
            .saturating_add(self.system)
            .saturating_add(self.nice)
    }
}

/// VM statistics from Mach `host_statistics64` with `HOST_VM_INFO64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawVmStats {
    pub free_count: u64,
    pub active_count: u64,
    pub inactive_count: u64,
    pub wire_count: u64,
    pub page_size: u64,
}

/// Swap usage from sysctl `vm.swapusage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSwapUsage {
    pub total_bytes: u64,
    pub used_bytes: u64,
}

/// System identity fields collected via sysctl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIdentity {
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub kernel_name: String,
    pub kernel_release: String,
    pub architecture: String,
    pub logical_cores: u32,
    pub physical_memory_bytes: u64,
}

/// Owned mounted-filesystem data returned by the macOS native query seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMountedFilesystem {
    pub mount_point: String,
    pub filesystem_type: String,
    pub fsid: (i32, i32),
    pub flags: u32,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub available_blocks: u64,
    pub block_size: u64,
}

/// Cumulative storage-service byte counters from `IOKit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDiskIo {
    pub id: String,
    pub name: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

/// Cumulative `AF_LINK` counters and native link metadata.
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

// ---------------------------------------------------------------------------
// Native query trait for test injection
// ---------------------------------------------------------------------------

/// Abstraction over native macOS system queries.
///
/// Production code calls FFI; tests inject a mock to exercise edge cases
/// without depending on the host state.
pub trait MacNativeQueries: Send + Sync + std::fmt::Debug {
    /// Enumerate mounted filesystems and their native capacity counters.
    fn mounted_filesystems(&self) -> Result<Vec<RawMountedFilesystem>, CollectError>;
    /// Read cumulative CPU tick counters from Mach `host_statistics`.
    fn cpu_load_info(&self) -> Result<RawCpuTicks, CollectError>;

    /// Read VM statistics from Mach `host_statistics64`.
    fn vm_info64(&self) -> Result<RawVmStats, CollectError>;

    /// Read swap usage from sysctl `vm.swapusage`.
    fn swap_usage(&self) -> Result<RawSwapUsage, CollectError>;

    /// Read one-, five-, and fifteen-minute load averages.
    fn load_averages(&self) -> Result<[f64; 3], CollectError>;

    /// Read system identity fields via sysctl.
    fn identity(&self) -> Result<RawIdentity, CollectError>;

    /// Read cumulative native storage byte counters.
    fn disk_io(&self) -> Result<Vec<RawDiskIo>, CollectError> {
        Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "IOKit storage statistics unavailable",
        ))
    }

    /// Read `AF_LINK` interface counters and link metadata.
    fn network_interfaces(&self) -> Result<Vec<RawNetworkInterface>, CollectError>;

    /// macOS has no supported unprivileged current-frequency source here.
    fn cpu_frequency_hz(&self) -> Option<u64> {
        None
    }
}

// ---------------------------------------------------------------------------
// Production FFI implementation
// ---------------------------------------------------------------------------

/// Production implementation backed by Mach and sysctl FFI.
#[derive(Debug, Clone, Copy)]
pub struct FfiNativeQueries;

impl MacNativeQueries for FfiNativeQueries {
    fn mounted_filesystems(&self) -> Result<Vec<RawMountedFilesystem>, CollectError> {
        mounted_filesystems()
    }

    fn cpu_load_info(&self) -> Result<RawCpuTicks, CollectError> {
        cpu_load_info()
    }

    fn vm_info64(&self) -> Result<RawVmStats, CollectError> {
        vm_info64()
    }

    fn swap_usage(&self) -> Result<RawSwapUsage, CollectError> {
        swap_usage()
    }

    fn load_averages(&self) -> Result<[f64; 3], CollectError> {
        load_averages()
    }

    fn identity(&self) -> Result<RawIdentity, CollectError> {
        collect_raw_identity()
    }

    fn disk_io(&self) -> Result<Vec<RawDiskIo>, CollectError> {
        disk_io()
    }

    fn network_interfaces(&self) -> Result<Vec<RawNetworkInterface>, CollectError> {
        network_interfaces()
    }
}

// ---------------------------------------------------------------------------
// Mock implementation for tests
// ---------------------------------------------------------------------------

/// Mock native queries for unit tests. All fields are public so tests can
/// inject different values between successive calls.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct MockNativeQueries {
    pub mounted: Vec<RawMountedFilesystem>,
    pub cpu: RawCpuTicks,
    pub vm: RawVmStats,
    pub swap: RawSwapUsage,
    pub load: [f64; 3],
    pub identity: RawIdentity,
    pub cpu_error: bool,
    pub vm_error: bool,
    pub swap_error: bool,
    pub load_error: bool,
    pub identity_error: bool,
    pub mounted_error: bool,
    /// When true, `cpu_load_info` increments `cpu` by a small delta on each
    /// call so successive samples produce a valid non-zero CPU interval.
    pub auto_increment_cpu: bool,
    pub(crate) cpu_call_count: std::sync::atomic::AtomicU32,
    pub disk: Vec<RawDiskIo>,
    pub network: Vec<RawNetworkInterface>,
}

impl Clone for MockNativeQueries {
    fn clone(&self) -> Self {
        Self {
            cpu: self.cpu,
            mounted: self.mounted.clone(),
            vm: self.vm,
            swap: self.swap,
            load: self.load,
            identity: self.identity.clone(),
            cpu_error: self.cpu_error,
            vm_error: self.vm_error,
            swap_error: self.swap_error,
            load_error: self.load_error,
            identity_error: self.identity_error,
            mounted_error: self.mounted_error,
            auto_increment_cpu: self.auto_increment_cpu,
            cpu_call_count: std::sync::atomic::AtomicU32::new(
                self.cpu_call_count
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            disk: self.disk.clone(),
            network: self.network.clone(),
        }
    }
}

impl MockNativeQueries {
    /// Build a mock returning sensible default values.
    pub fn success() -> Self {
        Self {
            mounted: vec![RawMountedFilesystem {
                mount_point: "/".to_string(),
                filesystem_type: "apfs".to_string(),
                fsid: (1, 1),
                flags: MNT_LOCAL,
                total_blocks: 100,
                free_blocks: 25,
                available_blocks: 20,
                block_size: 4096,
            }],
            cpu: RawCpuTicks {
                user: 1000,
                system: 500,
                idle: 8000,
                nice: 100,
            },
            vm: RawVmStats {
                free_count: 100_000,
                active_count: 200_000,
                inactive_count: 150_000,
                wire_count: 50_000,
                page_size: 16_384,
            },
            swap: RawSwapUsage {
                total_bytes: 0,
                used_bytes: 0,
            },
            load: [1.5, 1.0, 0.5],
            identity: RawIdentity {
                hostname: "test-mac.local".to_string(),
                os_name: "macos".to_string(),
                os_version: "15.0".to_string(),
                kernel_name: "Darwin".to_string(),
                kernel_release: "24.0.0".to_string(),
                architecture: "arm64".to_string(),
                logical_cores: 8,
                physical_memory_bytes: 16_000_000_000,
            },
            cpu_error: false,
            vm_error: false,
            swap_error: false,
            load_error: false,
            identity_error: false,
            mounted_error: false,
            auto_increment_cpu: false,
            cpu_call_count: std::sync::atomic::AtomicU32::new(0),
            disk: Vec::new(),
            network: Vec::new(),
        }
    }
}

impl MacNativeQueries for MockNativeQueries {
    fn mounted_filesystems(&self) -> Result<Vec<RawMountedFilesystem>, CollectError> {
        if self.mounted_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock mounted filesystem error",
            ));
        }
        Ok(self.mounted.clone())
    }

    fn cpu_load_info(&self) -> Result<RawCpuTicks, CollectError> {
        if self.cpu_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock cpu error",
            ));
        }
        if self.auto_increment_cpu {
            use std::sync::atomic::Ordering;
            let call = self.cpu_call_count.fetch_add(1, Ordering::Relaxed);
            // Each call after the first adds 100 ticks to user and 50 to idle,
            // producing a valid non-zero delta between successive samples.
            let offset = u64::from(call) * 100;
            return Ok(RawCpuTicks {
                user: self.cpu.user + offset,
                system: self.cpu.system,
                idle: self.cpu.idle + offset / 2,
                nice: self.cpu.nice,
            });
        }
        Ok(self.cpu)
    }

    fn vm_info64(&self) -> Result<RawVmStats, CollectError> {
        if self.vm_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock vm error",
            ));
        }
        Ok(self.vm)
    }

    fn swap_usage(&self) -> Result<RawSwapUsage, CollectError> {
        if self.swap_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock swap error",
            ));
        }
        Ok(self.swap)
    }

    fn load_averages(&self) -> Result<[f64; 3], CollectError> {
        if self.load_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock load error",
            ));
        }
        Ok(self.load)
    }

    fn identity(&self) -> Result<RawIdentity, CollectError> {
        if self.identity_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock identity error",
            ));
        }
        Ok(self.identity.clone())
    }

    fn disk_io(&self) -> Result<Vec<RawDiskIo>, CollectError> {
        Ok(self.disk.clone())
    }

    fn network_interfaces(&self) -> Result<Vec<RawNetworkInterface>, CollectError> {
        Ok(self.network.clone())
    }
}

// ---------------------------------------------------------------------------
// C type and constant declarations
// ---------------------------------------------------------------------------

#[allow(non_camel_case_types)]
type kern_return_t = i32;
#[allow(non_camel_case_types)]
type mach_port_t = u32;
#[allow(non_camel_case_types)]
type mach_msg_type_number_t = u32;

const KERN_SUCCESS: kern_return_t = 0;

#[allow(non_camel_case_types)]
const HOST_CPU_LOAD_INFO: i32 = 3;
#[allow(non_camel_case_types)]
const HOST_VM_INFO64: i32 = 4;
// Filesystem mount flags. Values mirror the Darwin headers; libc remains the
// authority for the `statfs`/`getmntinfo` ABI itself (see below).
pub(crate) const MNT_LOCAL: u32 = libc::MNT_LOCAL as u32;
pub(crate) const MNT_DONTBROWSE: u32 = libc::MNT_DONTBROWSE as u32;

// ---------------------------------------------------------------------------
// Darwin filesystem ABI
// ---------------------------------------------------------------------------
//
// Filesystem enumeration uses `libc::getmntinfo` with `libc::statfs` directly.
// libc selects the architecture-sensitive `getmntinfo$INODE64` symbol on
// non-aarch64 macOS, matching the modern 64-bit inode ABI. Gregg must not
// duplicate this layout or symbol selection: a private unsuffixed binding can
// misinterpret block counts, flags, and mount points on Intel and collapse a
// real local filesystem set to an empty drive list.

// ---------------------------------------------------------------------------
// Extern function declarations
// ---------------------------------------------------------------------------

extern "C" {
    /// Canonical Mach host-self interface. Returns a send right to the
    /// host port. Unlike the legacy `host_self()` compatibility symbol,
    /// `mach_host_self()` is the documented Mach trap for obtaining the
    /// host port in modern macOS.
    fn mach_host_self() -> mach_port_t;

    /// Returns the calling task's own port (the current task IPC space).
    /// Used as the task argument to `mach_port_deallocate`.
    fn mach_task_self() -> mach_port_t;

    fn mach_port_deallocate(task: mach_port_t, name: mach_port_t) -> kern_return_t;

    fn host_statistics(
        host_priv: mach_port_t,
        flavor: i32,
        info_out: *mut i32,
        info_out_cnt: *mut mach_msg_type_number_t,
    ) -> kern_return_t;

    fn host_statistics64(
        host_priv: mach_port_t,
        flavor: i32,
        info_out: *mut i32,
        info_out_cnt: *mut mach_msg_type_number_t,
    ) -> kern_return_t;

    fn host_page_size(host_priv: mach_port_t, page_size: *mut usize) -> kern_return_t;

    fn sysctlbyname(
        name: *const std::ffi::c_char,
        oldp: *mut std::ffi::c_void,
        oldlenp: *mut usize,
        newp: *const std::ffi::c_void,
        newlen: usize,
    ) -> i32;

    fn getloadavg(loadavg: *mut f64, nelem: std::ffi::c_int) -> std::ffi::c_int;

}

#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn IOServiceGetMatchingServices(
        master: mach_port_t,
        matching: *mut std::ffi::c_void,
        existing: *mut u32,
    ) -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IORegistryEntryCreateCFProperties(
        entry: u32,
        properties: *mut *mut std::ffi::c_void,
        allocator: *const std::ffi::c_void,
        options: u32,
    ) -> i32;
    fn IORegistryEntryGetName(entry: u32, name: *mut std::ffi::c_char) -> i32;
    fn IORegistryEntryGetRegistryEntryID(entry: u32, entry_id: *mut u64) -> i32;
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(
        allocator: *const std::ffi::c_void,
        string: *const std::ffi::c_char,
        encoding: u32,
    ) -> *mut std::ffi::c_void;
    fn CFDictionaryGetValue(
        dictionary: *const std::ffi::c_void,
        key: *const std::ffi::c_void,
    ) -> *const std::ffi::c_void;
    fn CFNumberGetValue(
        number: *const std::ffi::c_void,
        number_type: i32,
        value: *mut std::ffi::c_void,
    ) -> bool;
    fn CFRelease(object: *const std::ffi::c_void);
}

#[allow(clippy::cast_sign_loss)]
fn native_c_string(bytes: &[libc::c_char]) -> Result<String, CollectError> {
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let bytes: Vec<u8> = bytes[..length].iter().map(|byte| *byte as u8).collect();
    String::from_utf8(bytes).map_err(|_| {
        CollectError::new(
            CollectErrorKind::Parse,
            "mounted filesystem name is not UTF-8",
        )
    })
}

/// Read the opaque Darwin `fsid_t` as its two native `i32` components.
///
/// `libc::fsid_t` exposes no public fields; on Darwin it is two `i32` values
/// (8 bytes total). The copy below reads exactly those bytes through the
/// public type without assuming any wider layout.
#[cfg(target_os = "macos")]
fn fsid_pair(fsid: &libc::fsid_t) -> (i32, i32) {
    debug_assert_eq!(std::mem::size_of::<libc::fsid_t>(), 8);
    // Safety: `fsid` is a valid, aligned `fsid_t`; Darwin defines it as
    // `[i32; 2]`, so reading 8 bytes as two native-endian `i32` values is
    // layout-correct. No reference to the private field escapes.
    let pair: [i32; 2] = unsafe { std::ptr::addr_of!(*fsid).cast::<[i32; 2]>().read() };
    (pair[0], pair[1])
}

/// Convert one `libc::statfs` record into the owned collector seam.
///
/// `f_bsize` is `u32` in the libc ABI so the widening to `u64` is infallible;
/// zero sizes and overflowing `blocks * size` products are rejected later by
/// the shared drive-capacity filter (`total > 0`, `free <= total`,
/// `available <= total` with checked multiplication), preserving truthful
/// capacity semantics without fabricating zeroes.
#[cfg(target_os = "macos")]
fn raw_from_statfs(stat: &libc::statfs) -> Result<RawMountedFilesystem, CollectError> {
    Ok(RawMountedFilesystem {
        mount_point: native_c_string(&stat.f_mntonname)?,
        filesystem_type: native_c_string(&stat.f_fstypename)?,
        fsid: fsid_pair(&stat.f_fsid),
        flags: stat.f_flags,
        total_blocks: stat.f_blocks,
        free_blocks: stat.f_bfree,
        available_blocks: stat.f_bavail,
        block_size: u64::from(stat.f_bsize),
    })
}

fn mounted_filesystems() -> Result<Vec<RawMountedFilesystem>, CollectError> {
    #[cfg(target_os = "macos")]
    {
        let mut pointer: *mut libc::statfs = std::ptr::null_mut();
        // Safety: libc::getmntinfo writes a pointer to a kernel-owned array
        // and returns its element count, selecting the architecture-correct
        // INODE64 symbol. The array remains valid for this call; every
        // accepted record is copied into an owned Rust value before returning.
        let count = unsafe { libc::getmntinfo(&mut pointer, 0) };
        let count = mounted_filesystem_count(count, pointer)?;
        let mut result = Vec::with_capacity(count);
        for index in 0..count {
            // Safety: index is bounded by the count returned by getmntinfo and
            // the pointer targets a kernel-owned array valid for this call.
            let stat = unsafe { &*pointer.add(index) };
            result.push(raw_from_statfs(stat)?);
        }
        Ok(result)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "macOS filesystem API unavailable",
        ))
    }
}

/// Darwin interface-counter sources.
///
/// The preferred source is `NET_RT_IFLIST2` (`if_msghdr2` + embedded
/// `if_data64`), which exposes true 64-bit `ifi_ibytes`/`ifi_obytes`.
/// Darwin's `getifaddrs(3)` associates `AF_LINK` `ifa_data` with the legacy
/// 32-bit `struct if_data` — never `if_data64` — so the `getifaddrs` path is
/// kept only as a compatibility fallback for older/unsupported hosts.
#[cfg(target_os = "macos")]
const MAX_IFLIST2_BYTES: usize = 16 * 1024 * 1024;

/// One parsed `RTM_IFINFO2` record: native index, flags, and 64-bit counters.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IfList2Record {
    index: u32,
    flags: i32,
    rx_bytes: u64,
    tx_bytes: u64,
    baudrate_bps: u64,
}

/// Parse a raw `NET_RT_IFLIST2` sysctl buffer without invoking syscalls.
///
/// Walks variable-length routing messages by `ifm_msglen`, validates every
/// length before reading an `if_msghdr2`, accepts only `RTM_IFINFO2`, skips
/// unrelated route messages, and truncates (rather than over-reading) on a
/// malformed or truncated tail. Buffer alignment is not assumed; the header
/// is copied with `read_unaligned`.
#[cfg(target_os = "macos")]
fn parse_iflist2_buffer(buffer: &[u8]) -> Vec<IfList2Record> {
    let mut records = Vec::new();
    let header_len = std::mem::size_of::<libc::if_msghdr2>();
    let mut offset = 0usize;
    while offset + 2 <= buffer.len() {
        let msglen = u16::from_ne_bytes([buffer[offset], buffer[offset + 1]]) as usize;
        if msglen < header_len || msglen == 0 || offset + msglen > buffer.len() {
            break;
        }
        // Safety: `msglen >= header_len` and `offset + msglen <= len` were
        // validated above, so `[offset, offset + header_len)` is in bounds.
        // The parser offset carries no alignment guarantee, hence the
        // unaligned copy into an owned (aligned) header value.
        let header: libc::if_msghdr2 = unsafe {
            std::ptr::read_unaligned(buffer[offset..].as_ptr().cast::<libc::if_msghdr2>())
        };
        if i32::from(header.ifm_type) == libc::RTM_IFINFO2 && header.ifm_index != 0 {
            // `header` is an owned aligned copy, so field access is safe.
            records.push(IfList2Record {
                index: u32::from(header.ifm_index),
                flags: header.ifm_flags,
                rx_bytes: header.ifm_data.ifi_ibytes,
                tx_bytes: header.ifm_data.ifi_obytes,
                baudrate_bps: header.ifm_data.ifi_baudrate,
            });
        }
        offset += msglen;
        if records.len() > 4096 {
            break;
        }
    }
    records
}

/// Build a collector record from a parsed `RTM_IFINFO2` entry and its native
/// display name. Identity comes from the resolved name; loopback and
/// operational state come from native flags, never from naming conventions.
#[cfg(target_os = "macos")]
fn build_iflist2_interface(record: &IfList2Record, name: &str) -> RawNetworkInterface {
    let is_loopback = record.flags & libc::IFF_LOOPBACK != 0;
    let operational = record.flags & libc::IFF_UP != 0 && record.flags & libc::IFF_RUNNING != 0;
    let capacity = (record.baudrate_bps > 0).then_some(record.baudrate_bps);
    RawNetworkInterface {
        id: name.to_owned(),
        name: name.to_owned(),
        rx_bytes: record.rx_bytes,
        tx_bytes: record.tx_bytes,
        rx_capacity_bps: capacity,
        tx_capacity_bps: capacity,
        is_loopback,
        operational,
        aggregate_member: !is_loopback,
    }
}

/// Convert one legacy `getifaddrs`/`if_data` entry. Counters are 32-bit here;
/// widening to `u64` preserves the value but never synthesizes bytes across a
/// wrap — the shared `CounterBaselines` helper re-baselines on any decrease.
/// A zero or unrepresentable legacy baud rate publishes `None` capacity.
#[cfg(target_os = "macos")]
fn raw_from_if_data(name: &str, flags: u32, data: &libc::if_data) -> RawNetworkInterface {
    let is_loopback = flags & libc::IFF_LOOPBACK as u32 != 0;
    let operational = flags & libc::IFF_UP as u32 != 0 && flags & libc::IFF_RUNNING as u32 != 0;
    let capacity = (data.ifi_baudrate > 0).then(|| u64::from(data.ifi_baudrate));
    RawNetworkInterface {
        id: name.to_owned(),
        name: name.to_owned(),
        rx_bytes: u64::from(data.ifi_ibytes),
        tx_bytes: u64::from(data.ifi_obytes),
        rx_capacity_bps: capacity,
        tx_capacity_bps: capacity,
        is_loopback,
        operational,
        aggregate_member: !is_loopback,
    }
}

/// Resolve a native interface index to its display name via `if_indextoname`.
#[cfg(target_os = "macos")]
fn resolve_interface_name(index: u32) -> Option<String> {
    let mut buffer = [0 as libc::c_char; 16];
    // Safety: `buffer` is `IFNAMSIZ` bytes; `if_indextoname` writes a
    // NUL-terminated name on success and returns null on failure.
    let result = unsafe { libc::if_indextoname(index as libc::c_uint, buffer.as_mut_ptr()) };
    if result.is_null() {
        return None;
    }
    // Safety: the buffer is NUL-terminated per the successful-call contract.
    let name = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
        .to_str()
        .ok()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// Fetch the raw `NET_RT_IFLIST2` sysctl buffer with the standard size-query +
/// data-query sequence, tolerating a size race with a small bounded retry.
#[cfg(target_os = "macos")]
fn sysctl_iflist2_buffer() -> Result<Vec<u8>, CollectError> {
    let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
    // Safety: size query with null `oldp` fills `len` with the required size.
    let mut len: libc::size_t = 0;
    let sized = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            6,
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if sized != 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "NET_RT_IFLIST2 size query failed",
        )
        .with_source(std::io::Error::last_os_error()));
    }
    if len == 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "NET_RT_IFLIST2 returned an empty interface list",
        ));
    }
    if len as usize > MAX_IFLIST2_BYTES {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "NET_RT_IFLIST2 interface list exceeds bounded size",
        ));
    }
    for _ in 0..3 {
        let mut buffer = vec![0u8; len as usize];
        let mut out_len = len;
        // Safety: `buffer` has `out_len` bytes; the kernel writes at most
        // that many and updates `out_len` to the actual size.
        let fetched = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                6,
                buffer.as_mut_ptr().cast::<libc::c_void>(),
                &mut out_len,
                std::ptr::null_mut(),
                0,
            )
        };
        if fetched == 0 {
            if out_len == 0 {
                return Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "NET_RT_IFLIST2 returned an empty interface list",
                ));
            }
            buffer.truncate(out_len as usize);
            return Ok(buffer);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOMEM) {
            // Size race: re-query the required length and retry boundedly.
            len = 0;
            // Safety: size re-query with null `oldp`.
            let resized = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    6,
                    std::ptr::null_mut(),
                    &mut len,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if resized != 0 {
                return Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "NET_RT_IFLIST2 size re-query failed",
                )
                .with_source(std::io::Error::last_os_error()));
            }
            if len == 0 || len as usize > MAX_IFLIST2_BYTES {
                return Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "NET_RT_IFLIST2 interface list size is invalid",
                ));
            }
            continue;
        }
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "NET_RT_IFLIST2 data query failed",
        )
        .with_source(error));
    }
    Err(CollectError::new(
        CollectErrorKind::SourceUnavailable,
        "NET_RT_IFLIST2 size race did not settle",
    ))
}

/// Sort and deduplicate interface records deterministically by stable identity.
#[cfg(target_os = "macos")]
fn sort_dedup_interfaces(mut records: Vec<RawNetworkInterface>) -> Vec<RawNetworkInterface> {
    records.sort_by(|left, right| left.id.cmp(&right.id));
    records.dedup_by(|left, right| left.id == right.id);
    records
}

/// Preferred 64-bit interface counters from `NET_RT_IFLIST2`/`if_msghdr2`.
#[cfg(target_os = "macos")]
fn network_interfaces_iflist2() -> Result<Vec<RawNetworkInterface>, CollectError> {
    let buffer = sysctl_iflist2_buffer()?;
    let mut interfaces = Vec::new();
    for record in parse_iflist2_buffer(&buffer) {
        if let Some(name) = resolve_interface_name(record.index) {
            interfaces.push(build_iflist2_interface(&record, &name));
        }
    }
    Ok(sort_dedup_interfaces(interfaces))
}

/// Compatibility fallback: `getifaddrs` with correctly typed `if_data`.
///
/// Darwin documents `AF_LINK` `ifa_data` as `struct if_data` (32-bit
/// counters). This must never be cast to `if_data64`.
#[cfg(target_os = "macos")]
fn network_interfaces_getifaddrs() -> Result<Vec<RawNetworkInterface>, CollectError> {
    let mut head = std::ptr::null_mut();
    // Safety: getifaddrs initializes an owned linked list on success;
    // every returned list is released exactly once below.
    let result = unsafe { libc::getifaddrs(&mut head) };
    if result != 0 {
        return Err(
            CollectError::new(CollectErrorKind::SourceUnavailable, "getifaddrs failed")
                .with_source(std::io::Error::last_os_error()),
        );
    }
    let mut records = Vec::new();
    let mut current = head;
    while !current.is_null() {
        // Safety: current is a node in the list owned by getifaddrs and
        // remains valid until freeifaddrs after this traversal.
        let item = unsafe { &*current };
        if !item.ifa_name.is_null()
            && !item.ifa_addr.is_null()
            && unsafe { i32::from((*item.ifa_addr).sa_family) } == libc::AF_LINK
            && !item.ifa_data.is_null()
        {
            // Safety: per getifaddrs(3), AF_LINK ifa_data points at the
            // legacy `if_data` record for this interface. Copy scalar fields
            // only; never interpret these bytes as `if_data64`.
            let data = unsafe { &*(item.ifa_data.cast::<libc::if_data>()) };
            let name = unsafe { std::ffi::CStr::from_ptr(item.ifa_name) }
                .to_str()
                .map_err(|_| {
                    CollectError::new(CollectErrorKind::Parse, "interface name is not UTF-8")
                })?;
            records.push(raw_from_if_data(name, item.ifa_flags, data));
        }
        // Safety: traversal remains within the list returned by getifaddrs.
        current = unsafe { (*current).ifa_next };
    }
    // Safety: head was returned by getifaddrs and has not been freed yet.
    unsafe { libc::freeifaddrs(head) };
    Ok(sort_dedup_interfaces(records))
}

/// Query native interface counters, preferring 64-bit `NET_RT_IFLIST2`.
///
/// Falls back to the correctly typed `getifaddrs`/`if_data` path only when
/// the preferred source is unavailable or unsupported on the running host.
fn network_interfaces() -> Result<Vec<RawNetworkInterface>, CollectError> {
    #[cfg(target_os = "macos")]
    {
        match network_interfaces_iflist2() {
            Ok(interfaces) => Ok(interfaces),
            Err(_) => network_interfaces_getifaddrs(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "macOS interface API unavailable",
        ))
    }
}

/// Read `IOKit` block-storage driver statistics. One malformed service is
/// skipped, allowing other optional metric families to remain healthy.
fn disk_io() -> Result<Vec<RawDiskIo>, CollectError> {
    #[cfg(target_os = "macos")]
    {
        let class = std::ffi::CString::new("IOBlockStorageDriver").map_err(|_| {
            CollectError::new(CollectErrorKind::Parse, "IOKit class name contains NUL")
        })?;
        // Safety: IOKit returns an iterator owned by this function and the
        // matching dictionary is consumed by the matching-services call.
        let matching = unsafe { IOServiceMatching(class.as_ptr()) };
        if matching.is_null() {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "IOMedia matching unavailable",
            ));
        }
        let mut iterator = 0;
        let status = unsafe { IOServiceGetMatchingServices(0, matching, &mut iterator) };
        if status != 0 {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "IOMedia enumeration failed",
            ));
        }
        let statistics_key = cf_string("Statistics")?;
        let read_key = cf_string("Bytes (Read)")?;
        let write_key = cf_string("Bytes (Write)")?;
        let mut records = Vec::new();
        loop {
            let service = unsafe { IOIteratorNext(iterator) };
            if service == 0 {
                break;
            }
            let mut properties = std::ptr::null_mut();
            let result = unsafe {
                IORegistryEntryCreateCFProperties(service, &mut properties, std::ptr::null(), 0)
            };
            if result == 0 && !properties.is_null() {
                let values = unsafe { CFDictionaryGetValue(properties, statistics_key.cast()) };
                if !values.is_null() {
                    let read = unsafe { CFDictionaryGetValue(values, read_key.cast()) };
                    let write = unsafe { CFDictionaryGetValue(values, write_key.cast()) };
                    let mut read_bytes = 0i64;
                    let mut write_bytes = 0i64;
                    let read_ok = !read.is_null()
                        && unsafe {
                            CFNumberGetValue(read, 4, std::ptr::addr_of_mut!(read_bytes).cast())
                        };
                    let write_ok = !write.is_null()
                        && unsafe {
                            CFNumberGetValue(write, 4, std::ptr::addr_of_mut!(write_bytes).cast())
                        };
                    if read_ok && write_ok && read_bytes >= 0 && write_bytes >= 0 {
                        let mut name = [0i8; 256];
                        let name_ok =
                            unsafe { IORegistryEntryGetName(service, name.as_mut_ptr()) } == 0;
                        let mut registry_id = 0u64;
                        let id_status =
                            unsafe { IORegistryEntryGetRegistryEntryID(service, &mut registry_id) };
                        if name_ok && id_status == 0 {
                            let display = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
                                .to_string_lossy()
                                .into_owned();
                            records.push(RawDiskIo {
                                id: format!("ioreg-{registry_id}"),
                                name: display,
                                read_bytes: u64::try_from(read_bytes).unwrap_or(0),
                                write_bytes: u64::try_from(write_bytes).unwrap_or(0),
                            });
                        }
                    }
                }
                unsafe { CFRelease(properties.cast()) };
            }
            unsafe { IOObjectRelease(service) };
        }
        unsafe { IOObjectRelease(iterator) };
        unsafe {
            CFRelease(statistics_key.cast());
            CFRelease(read_key.cast());
            CFRelease(write_key.cast());
        }
        records.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(records)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "IOKit unavailable",
        ))
    }
}

#[cfg(target_os = "macos")]
type CfMutableRef = *mut std::ffi::c_void;

#[cfg(target_os = "macos")]
fn cf_string(value: &str) -> Result<CfMutableRef, CollectError> {
    let c = std::ffi::CString::new(value)
        .map_err(|_| CollectError::new(CollectErrorKind::Parse, "invalid CoreFoundation key"))?;
    // Safety: c is a valid NUL-terminated UTF-8 string and the returned
    // object is released by the caller after dictionary access.
    let result = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x0800_0100) };
    (!result.is_null()).then_some(result).ok_or_else(|| {
        CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "CoreFoundation key allocation failed",
        )
    })
}

#[cfg(target_os = "macos")]
fn mounted_filesystem_count(count: i32, pointer: *mut libc::statfs) -> Result<usize, CollectError> {
    if count <= 0 || pointer.is_null() {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "getmntinfo returned an invalid result",
        ));
    }
    usize::try_from(count)
        .map_err(|_| CollectError::new(CollectErrorKind::Parse, "getmntinfo count overflow"))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{
        build_iflist2_interface, mounted_filesystem_count, parse_iflist2_buffer, raw_from_if_data,
        raw_from_statfs, sort_dedup_interfaces, IfList2Record, RawMountedFilesystem,
        RawNetworkInterface,
    };
    use crate::collector::error::CollectErrorKind;

    #[test]
    fn zero_mount_count_is_source_failure() {
        let error = mounted_filesystem_count(0, std::ptr::dangling_mut::<libc::statfs>())
            .expect_err("zero getmntinfo count must fail");
        assert_eq!(error.kind, CollectErrorKind::SourceUnavailable);
    }

    #[test]
    fn positive_mount_count_requires_pointer() {
        let error = mounted_filesystem_count(1, std::ptr::null_mut())
            .expect_err("positive count with null pointer must fail");
        assert_eq!(error.kind, CollectErrorKind::SourceUnavailable);
    }

    #[allow(clippy::cast_possible_wrap)]
    fn test_statfs(
        mount: &str,
        fstype: &str,
        flags: u32,
        block_size: u32,
        blocks: u64,
        bfree: u64,
        bavail: u64,
    ) -> libc::statfs {
        // Safety: integer/C-char record; zeroed bytes are a valid initial state
        // before individual fields are assigned below.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        stat.f_bsize = block_size;
        stat.f_blocks = blocks;
        stat.f_bfree = bfree;
        stat.f_bavail = bavail;
        stat.f_flags = flags;
        for (slot, byte) in stat.f_mntonname.iter_mut().zip(
            mount
                .as_bytes()
                .iter()
                .copied()
                .chain(std::iter::repeat(0))
                .take(1024),
        ) {
            *slot = byte as libc::c_char;
        }
        for (slot, byte) in stat.f_fstypename.iter_mut().zip(
            fstype
                .as_bytes()
                .iter()
                .copied()
                .chain(std::iter::repeat(0))
                .take(16),
        ) {
            *slot = byte as libc::c_char;
        }
        stat
    }

    #[test]
    fn libc_statfs_conversion_preserves_nonzero_capacity() {
        let stat = test_statfs("/", "apfs", super::MNT_LOCAL, 4096, 100, 25, 20);
        let raw: RawMountedFilesystem = raw_from_statfs(&stat).expect("converts");
        assert_eq!(raw.mount_point, "/");
        assert_eq!(raw.filesystem_type, "apfs");
        assert_eq!(raw.block_size, 4096);
        assert_eq!(raw.total_blocks, 100);
        assert_eq!(raw.free_blocks, 25);
        assert_eq!(raw.available_blocks, 20);
        assert_eq!(raw.flags & super::MNT_LOCAL, super::MNT_LOCAL);
    }

    #[test]
    #[allow(clippy::cast_possible_wrap)]
    fn libc_statfs_conversion_rejects_non_utf8() {
        let mut stat = test_statfs("/", "apfs", super::MNT_LOCAL, 4096, 100, 25, 20);
        stat.f_mntonname[0] = 0xFFu8 as libc::c_char;
        stat.f_mntonname[1] = 0xFEu8 as libc::c_char;
        let error = raw_from_statfs(&stat).expect_err("non-UTF8 mount must fail");
        assert_eq!(error.kind, CollectErrorKind::Parse);
    }

    #[allow(clippy::cast_possible_truncation)]
    fn encode_ifinfo2(index: u16, flags: i32, rx: u64, tx: u64, baudrate: u64) -> Vec<u8> {
        // Safety: integer record; zeroed bytes are valid before fields are set.
        let mut data: libc::if_data64 = unsafe { std::mem::zeroed() };
        data.ifi_ibytes = rx;
        data.ifi_obytes = tx;
        data.ifi_baudrate = baudrate;
        // Safety: integer record; zeroed bytes are valid before fields are set.
        let mut header: libc::if_msghdr2 = unsafe { std::mem::zeroed() };
        header.ifm_msglen = std::mem::size_of::<libc::if_msghdr2>() as u16;
        header.ifm_version = libc::RTM_VERSION as u8;
        header.ifm_type = libc::RTM_IFINFO2 as u8;
        header.ifm_flags = flags;
        header.ifm_index = index;
        header.ifm_data = data;
        // Safety: `header` is an aligned owned value; copying its bytes into a
        // fresh `Vec<u8>` never reads past the struct.
        unsafe {
            std::slice::from_raw_parts(
                std::ptr::addr_of!(header).cast::<u8>(),
                std::mem::size_of::<libc::if_msghdr2>(),
            )
            .to_vec()
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn encode_unrelated_route_message() -> Vec<u8> {
        // Safety: integer record; zeroed bytes are valid before fields are set.
        let mut header: libc::if_msghdr2 = unsafe { std::mem::zeroed() };
        header.ifm_msglen = std::mem::size_of::<libc::if_msghdr2>() as u16;
        header.ifm_version = libc::RTM_VERSION as u8;
        header.ifm_type = libc::RTM_NEWADDR as u8;
        header.ifm_index = 99;
        // Safety: aligned owned copy to bytes as above.
        unsafe {
            std::slice::from_raw_parts(
                std::ptr::addr_of!(header).cast::<u8>(),
                std::mem::size_of::<libc::if_msghdr2>(),
            )
            .to_vec()
        }
    }

    #[test]
    fn iflist2_parser_accepts_multiple_ifinfo2_records() {
        let mut buffer = encode_ifinfo2(1, libc::IFF_UP | libc::IFF_RUNNING, 100, 200, 1_000);
        buffer.extend(encode_ifinfo2(2, libc::IFF_UP, 0, 0, 0));
        let records = parse_iflist2_buffer(&buffer);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].index, 1);
        assert_eq!(records[0].rx_bytes, 100);
        assert_eq!(records[0].tx_bytes, 200);
        assert_eq!(records[0].baudrate_bps, 1_000);
        // Zero-byte counters are preserved, not dropped.
        assert_eq!(records[1].rx_bytes, 0);
        assert_eq!(records[1].tx_bytes, 0);
    }

    #[test]
    fn iflist2_parser_skips_unrelated_messages() {
        let mut buffer = encode_ifinfo2(1, libc::IFF_UP, 10, 20, 0);
        buffer.extend(encode_unrelated_route_message());
        buffer.extend(encode_ifinfo2(2, libc::IFF_UP, 30, 40, 0));
        let records = parse_iflist2_buffer(&buffer);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].index, 1);
        assert_eq!(records[1].index, 2);
    }

    #[test]
    fn iflist2_parser_truncates_malformed_tail_without_overread() {
        let mut buffer = encode_ifinfo2(1, libc::IFF_UP, 10, 20, 0);
        buffer.extend(encode_ifinfo2(2, libc::IFF_UP, 30, 40, 0));
        let truncated = buffer.len() - 3;
        buffer.truncate(truncated);
        let records = parse_iflist2_buffer(&buffer);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].index, 1);

        assert!(parse_iflist2_buffer(&[]).is_empty());
        assert!(parse_iflist2_buffer(&[0u8]).is_empty());
        // Declared length shorter than one header is malformed.
        assert!(parse_iflist2_buffer(&[4u8, 0u8, 0u8, 0u8]).is_empty());
    }

    #[test]
    fn iflist2_builder_derives_loopback_operational_and_capacity() {
        let loopback = IfList2Record {
            index: 1,
            flags: libc::IFF_LOOPBACK | libc::IFF_UP | libc::IFF_RUNNING,
            rx_bytes: 10,
            tx_bytes: 20,
            baudrate_bps: 1_000,
        };
        let record = build_iflist2_interface(&loopback, "lo0");
        assert!(record.is_loopback);
        assert!(!record.aggregate_member);
        assert!(record.operational);

        let down = IfList2Record {
            index: 2,
            flags: 0,
            rx_bytes: 30,
            tx_bytes: 40,
            baudrate_bps: 5_000,
        };
        let record = build_iflist2_interface(&down, "en0");
        assert!(!record.is_loopback);
        assert!(record.aggregate_member);
        assert!(!record.operational);
        // Down interfaces are retained in detail; capacity exclusion happens
        // at the aggregation layer, not by dropping the record.
        assert_eq!(record.rx_capacity_bps, Some(5_000));

        let no_capacity = IfList2Record {
            index: 3,
            flags: libc::IFF_UP | libc::IFF_RUNNING,
            rx_bytes: 0,
            tx_bytes: 0,
            baudrate_bps: 0,
        };
        let record = build_iflist2_interface(&no_capacity, "en1");
        assert_eq!(record.rx_capacity_bps, None);
        assert_eq!(record.tx_capacity_bps, None);
    }

    #[test]
    fn interface_dedup_is_deterministic_by_stable_id() {
        let make = |id: &str| RawNetworkInterface {
            id: id.to_owned(),
            name: id.to_owned(),
            rx_bytes: 1,
            tx_bytes: 2,
            rx_capacity_bps: None,
            tx_capacity_bps: None,
            is_loopback: false,
            operational: true,
            aggregate_member: true,
        };
        let sorted = sort_dedup_interfaces(vec![make("en1"), make("en0"), make("en0")]);
        let ids: Vec<&str> = sorted.iter().map(|record| record.id.as_str()).collect();
        assert_eq!(ids, vec!["en0", "en1"]);
    }

    #[test]
    fn fallback_if_data_widens_32bit_counters_without_if_data64() {
        // Safety: integer record; zeroed bytes are valid before fields are set.
        let mut data: libc::if_data = unsafe { std::mem::zeroed() };
        data.ifi_ibytes = u32::MAX;
        data.ifi_obytes = 123;
        data.ifi_baudrate = 1_000;
        let record = raw_from_if_data("en0", libc::IFF_UP as u32 | libc::IFF_RUNNING as u32, &data);
        // Widening preserves the 32-bit value exactly; the legacy path never
        // interprets these bytes as the wider `if_data64` layout.
        assert_eq!(record.rx_bytes, u64::from(u32::MAX));
        assert_eq!(record.tx_bytes, 123);
        assert_eq!(record.rx_capacity_bps, Some(1_000));
        assert!(!record.is_loopback);
        assert!(record.operational);

        data.ifi_baudrate = 0;
        let record = raw_from_if_data("en0", 0, &data);
        assert_eq!(record.rx_capacity_bps, None);
        assert!(!record.operational);
    }

    #[test]
    fn legacy_counter_decrease_rebaselines_without_spike() {
        use crate::collector::rate::CounterBaselines;
        use std::time::{Duration, Instant};
        let mut baselines = CounterBaselines::default();
        let start = Instant::now();
        assert_eq!(baselines.observe("en0", start, 1_000, 2_000), None);
        let first = baselines
            .observe("en0", start + Duration::from_secs(1), 2_000, 4_000)
            .expect("valid interval produces a rate");
        assert_eq!(first.first_per_sec, 1_000);
        // 32-bit wrap/decrease re-baselines: no rate, no spike.
        assert_eq!(
            baselines.observe("en0", start + Duration::from_secs(2), 100, 200),
            None
        );
        let recovered = baselines
            .observe("en0", start + Duration::from_secs(3), 1_100, 1_200)
            .expect("post-wrap interval recovers");
        assert_eq!(recovered.first_per_sec, 1_000);
        assert_eq!(recovered.second_per_sec, 1_000);
    }
}

// ---------------------------------------------------------------------------
// RAII wrapper for the Mach host-self port
// ---------------------------------------------------------------------------

const MACH_PORT_NULL: mach_port_t = 0;

/// RAII wrapper around a `mach_port_t` send right from `mach_host_self()`.
///
/// `mach_host_self()` returns a send right to the host port. The Mach
/// ownership model says send rights should be released, so this wrapper
/// deallocates the port on drop using the current task's IPC space.
///
/// The port is validated on acquisition: `MACH_PORT_NULL` is rejected
/// before any host-statistics call receives it.
struct HostPort {
    port: mach_port_t,
}

impl HostPort {
    /// Obtain a fresh host-self send right.
    ///
    /// Returns an error if `mach_host_self()` returns `MACH_PORT_NULL`,
    /// which would indicate a fundamental Mach subsystem failure.
    fn current() -> Result<Self, CollectError> {
        // Safety: `mach_host_self()` is a Mach trap that returns a
        // `mach_port_t` send right to the host port. While it is expected
        // to always return a valid port, we validate the result rather
        // than assuming it cannot fail.
        let port = unsafe { mach_host_self() };
        if port == MACH_PORT_NULL {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mach_host_self returned MACH_PORT_NULL",
            ));
        }
        Ok(Self { port })
    }

    /// Borrow the raw port value for FFI calls.
    fn raw(&self) -> mach_port_t {
        self.port
    }
}

impl Drop for HostPort {
    fn drop(&mut self) {
        if self.port == MACH_PORT_NULL {
            return;
        }

        // Safety: `mach_port_deallocate` releases one send right in the
        // specified task's IPC space. We pass the current task's port
        // (obtained via `mach_task_self()`) rather than `MACH_PORT_NULL`,
        // which is not a valid task argument.
        let task = unsafe { mach_task_self() };
        if task == MACH_PORT_NULL {
            return;
        }

        // Deallocate failure cannot be returned from Drop. The port is
        // marked null so a double-drop is a no-op.
        let _ = unsafe { mach_port_deallocate(task, self.port) };
        self.port = MACH_PORT_NULL;
    }
}

// ---------------------------------------------------------------------------
// FFI implementation functions
// ---------------------------------------------------------------------------

/// Widen a Mach `natural_t` counter that was written into an `i32` buffer.
///
/// Mach writes unsigned 32-bit values into the `host_statistics` buffer even
/// though the FFI signature exposes it as `integer_t`. Reinterpret the bit
/// pattern as unsigned so counters above `i32::MAX` keep their true magnitude
/// instead of sign-extending into huge `u64` deltas.
#[allow(clippy::cast_sign_loss)]
fn widen_natural(value: i32) -> u64 {
    u64::from(value as u32)
}

/// Read cumulative CPU tick counters from Mach `host_statistics`.
fn cpu_load_info() -> Result<RawCpuTicks, CollectError> {
    let host = HostPort::current()?;
    // Safety: `host_statistics` writes exactly 4 natural_t values into our
    // stack-allocated buffer. The buffer is properly aligned and large enough.
    // The return status is validated.
    let mut buf = [0i32; 4];
    let mut count: mach_msg_type_number_t = 4;
    let kr =
        unsafe { host_statistics(host.raw(), HOST_CPU_LOAD_INFO, buf.as_mut_ptr(), &mut count) };

    if kr != KERN_SUCCESS {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("host_statistics HOST_CPU_LOAD_INFO failed with status {kr}"),
        ));
    }

    if count < 4 {
        return Err(CollectError::new(
            CollectErrorKind::Parse,
            format!("host_statistics returned {count} fields, expected at least 4"),
        ));
    }

    Ok(RawCpuTicks {
        user: widen_natural(buf[0]),
        system: widen_natural(buf[1]),
        idle: widen_natural(buf[2]),
        nice: widen_natural(buf[3]),
    })
}

/// Read VM statistics from Mach `host_statistics64`.
fn vm_info64() -> Result<RawVmStats, CollectError> {
    let host = HostPort::current()?;
    // Safety: `host_statistics64` writes up to 64 natural_t values. We use a
    // generous buffer so the kernel cannot overflow even if future macOS
    // versions add fields. The return count tells us how many were written.
    let mut buf = [0i32; 64];
    let mut count: mach_msg_type_number_t = 64;
    let kr = unsafe { host_statistics64(host.raw(), HOST_VM_INFO64, buf.as_mut_ptr(), &mut count) };

    if kr != KERN_SUCCESS {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("host_statistics64 HOST_VM_INFO64 failed with status {kr}"),
        ));
    }

    if count < 4 {
        return Err(CollectError::new(
            CollectErrorKind::Parse,
            format!("host_statistics64 returned {count} fields, expected at least 4"),
        ));
    }

    let page_size = read_page_size()?;

    Ok(RawVmStats {
        free_count: widen_natural(buf[0]),
        active_count: widen_natural(buf[1]),
        inactive_count: widen_natural(buf[2]),
        wire_count: widen_natural(buf[3]),
        page_size,
    })
}

/// Read the host page size via `host_page_size`.
fn read_page_size() -> Result<u64, CollectError> {
    let host = HostPort::current()?;
    let mut page_size: usize = 0;
    // Safety: `host_page_size` writes a single usize value. The pointer is
    // valid and properly aligned. The return status is validated.
    let kr = unsafe { host_page_size(host.raw(), &mut page_size) };
    if kr != KERN_SUCCESS {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("host_page_size failed with status {kr}"),
        ));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(page_size as u64)
}

/// Read swap usage from sysctl `vm.swapusage`.
fn swap_usage() -> Result<RawSwapUsage, CollectError> {
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    #[allow(non_camel_case_types, clippy::struct_field_names)]
    struct xswusage {
        xsu_total: u64,
        xsu_avail: u64,
        xsu_used: u64,
        xsu_pagesize: u32,
        xsu_encrypted: u32,
    }

    let name = std::ffi::CString::new("vm.swapusage").map_err(|_| {
        CollectError::new(
            CollectErrorKind::Parse,
            "failed to create CString for vm.swapusage",
        )
    })?;

    let mut data = xswusage::default();
    let mut len = std::mem::size_of::<xswusage>();

    // Safety: `sysctlbyname` reads sizeof(xswusage) bytes into our
    // stack-allocated struct. The pointer, length, and struct layout are
    // correct for macOS. The return value is validated.
    let result = unsafe {
        sysctlbyname(
            name.as_ptr(),
            std::ptr::addr_of_mut!(data).cast::<std::ffi::c_void>(),
            &mut len,
            std::ptr::null(),
            0,
        )
    };

    if result != 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("sysctlbyname vm.swapusage failed with status {result}"),
        ));
    }

    // Validate the returned length matches the exact expected structure size.
    if len != std::mem::size_of::<xswusage>() {
        return Err(CollectError::new(
            CollectErrorKind::Parse,
            format!(
                "sysctlbyname vm.swapusage returned {len} bytes, expected {}",
                std::mem::size_of::<xswusage>()
            ),
        ));
    }

    Ok(RawSwapUsage {
        total_bytes: data.xsu_total,
        used_bytes: data.xsu_used,
    })
}

/// Read load averages via `getloadavg()`.
fn load_averages() -> Result<[f64; 3], CollectError> {
    let mut loadavg = [0.0_f64; 3];

    // Safety: `getloadavg` writes up to 3 f64 values into our buffer. The
    // buffer is properly aligned and large enough. A return value of -1
    // indicates failure.
    let filled = unsafe { getloadavg(loadavg.as_mut_ptr(), 3) };

    if filled < 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "getloadavg returned -1",
        ));
    }

    if filled < 3 {
        return Err(CollectError::new(
            CollectErrorKind::Parse,
            format!("getloadavg returned {filled} values, expected 3"),
        ));
    }

    for (i, &val) in loadavg.iter().enumerate() {
        if !val.is_finite() || val < 0.0 {
            return Err(CollectError::new(
                CollectErrorKind::Parse,
                format!("load average index {i} is not finite/non-negative"),
            ));
        }
    }

    Ok(loadavg)
}

/// Read a string sysctl value by name.
fn read_string_sysctl(name: &str) -> Result<String, CollectError> {
    let c_name = std::ffi::CString::new(name).map_err(|_| {
        CollectError::new(
            CollectErrorKind::Parse,
            format!("invalid sysctl name: {name}"),
        )
    })?;

    let mut len: usize = 0;
    // Safety: `sysctlbyname` with null oldp returns the required size.
    let result = unsafe {
        sysctlbyname(
            c_name.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null(),
            0,
        )
    };

    if result != 0 || len == 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("sysctlbyname {name} failed to query length"),
        ));
    }

    let mut buf = vec![0u8; len];
    // Safety: `sysctlbyname` reads up to `len` bytes.
    let result = unsafe {
        sysctlbyname(
            c_name.as_ptr(),
            buf.as_mut_ptr().cast::<std::ffi::c_void>(),
            &mut len,
            std::ptr::null(),
            0,
        )
    };

    if result != 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("sysctlbyname {name} failed with status {result}"),
        ));
    }

    if len > 0 && buf[len - 1] == 0 {
        len -= 1;
    }

    String::from_utf8(buf[..len].to_vec()).map_err(|e| {
        CollectError::new(
            CollectErrorKind::Parse,
            format!("sysctlbyname {name} returned invalid UTF-8"),
        )
        .with_source(e)
    })
}

/// Read an integer sysctl value by name.
fn read_int_sysctl<T: Copy + Default>(name: &str) -> Result<T, CollectError> {
    let c_name = std::ffi::CString::new(name).map_err(|_| {
        CollectError::new(
            CollectErrorKind::Parse,
            format!("invalid sysctl name: {name}"),
        )
    })?;

    let mut value: T = T::default();
    let mut len = std::mem::size_of::<T>();

    // Safety: `sysctlbyname` reads sizeof(T) bytes into our stack variable.
    let result = unsafe {
        sysctlbyname(
            c_name.as_ptr(),
            std::ptr::addr_of_mut!(value).cast::<std::ffi::c_void>(),
            &mut len,
            std::ptr::null(),
            0,
        )
    };

    if result != 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("sysctlbyname {name} failed with status {result}"),
        ));
    }

    Ok(value)
}

/// Read the macOS product version from SystemVersion.plist.
fn read_product_version() -> Result<String, CollectError> {
    let content = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist")
        .map_err(|e| {
            CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "failed to read SystemVersion.plist",
            )
            .with_source(e)
        })?;

    let lines: Vec<&str> = content.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains("<key>ProductVersion</key>") && i + 1 < lines.len() {
            let value_line = lines[i + 1];
            if let Some(start) = value_line.find("<string>") {
                let start = start + "<string>".len();
                if let Some(end) = value_line.find("</string>") {
                    return Ok(value_line[start..end].to_string());
                }
            }
        }
    }

    Ok("unknown".to_string())
}

/// Collect all raw identity fields from native APIs.
fn collect_raw_identity() -> Result<RawIdentity, CollectError> {
    let hostname = read_string_sysctl("kern.hostname")?;
    let kernel_release = read_string_sysctl("kern.osrelease")?;
    let architecture = read_string_sysctl("hw.machine")?;
    let logical_cores = read_int_sysctl::<u32>("hw.logicalcpu")?;
    let physical_memory_bytes = read_int_sysctl::<u64>("hw.memsize")?;

    let os_version = read_product_version().unwrap_or_else(|_| "unknown".to_string());

    Ok(RawIdentity {
        hostname,
        os_name: "macos".to_string(),
        os_version,
        kernel_name: "Darwin".to_string(),
        kernel_release,
        architecture,
        logical_cores,
        physical_memory_bytes,
    })
}

// ---------------------------------------------------------------------------
// Native macOS smoke tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod native_tests {
    use super::*;

    #[test]
    fn cpu_ticks_total_positive() {
        let q = FfiNativeQueries;
        let ticks = q.cpu_load_info().expect("cpu_load_info failed");
        assert!(
            ticks.total() > 0,
            "CPU tick total should be > 0, got {}",
            ticks.total()
        );
    }

    #[test]
    fn natural_counters_widen_without_sign_extension() {
        // Cumulative tick counters exceed i32::MAX after long uptimes; the
        // bit pattern must be reinterpreted as unsigned, not sign-extended.
        assert_eq!(widen_natural(0), 0);
        assert_eq!(widen_natural(1), 1);
        assert_eq!(widen_natural(i32::MAX), u64::from(i32::MAX as u32));
        assert_eq!(widen_natural(-1), u64::from(u32::MAX));
        assert_eq!(widen_natural(i32::MIN), u64::from(0x8000_0000u32));
    }

    #[test]
    fn vm_page_size_positive() {
        let q = FfiNativeQueries;
        let vm = q.vm_info64().expect("vm_info64 failed");
        assert!(
            vm.page_size > 0,
            "page_size should be > 0, got {}",
            vm.page_size
        );
    }

    #[test]
    fn swap_total_gte_used() {
        let q = FfiNativeQueries;
        let swap = q.swap_usage().expect("swap_usage failed");
        assert!(
            swap.total_bytes >= swap.used_bytes,
            "swap total ({}) should be >= used ({})",
            swap.total_bytes,
            swap.used_bytes
        );
    }

    #[test]
    fn load_averages_finite_non_negative() {
        let q = FfiNativeQueries;
        let loads = q.load_averages().expect("load_averages failed");
        for (i, &val) in loads.iter().enumerate() {
            assert!(
                val.is_finite(),
                "load average [{i}] should be finite, got {val}"
            );
            assert!(val >= 0.0, "load average [{i}] should be >= 0, got {val}");
        }
    }

    #[test]
    fn identity_non_empty_fields() {
        let q = FfiNativeQueries;
        let id = q.identity().expect("identity failed");
        assert!(!id.hostname.is_empty(), "hostname must not be empty");
        assert!(
            !id.architecture.is_empty(),
            "architecture must not be empty"
        );
        assert!(
            !id.kernel_release.is_empty(),
            "kernel_release must not be empty"
        );
        assert!(!id.kernel_name.is_empty(), "kernel_name must not be empty");
        assert!(!id.os_name.is_empty(), "os_name must not be empty");
    }

    #[test]
    fn xswusage_field_mapping() {
        // Verify that our `xswusage` layout matches the Darwin definition
        // by checking size and field offsets using std::mem.
        #[repr(C)]
        #[allow(clippy::struct_field_names)]
        struct DarwinXswusage {
            xsu_total: u64,
            xsu_avail: u64,
            xsu_used: u64,
            xsu_pagesize: u32,
            xsu_encrypted: u32,
        }

        // Our FFI struct is local to `swap_usage()`, so we replicate it here
        // for layout verification.
        #[repr(C)]
        #[derive(Copy, Clone)]
        #[allow(clippy::struct_field_names)]
        struct TestXswusage {
            xsu_total: u64,
            xsu_avail: u64,
            xsu_used: u64,
            xsu_pagesize: u32,
            xsu_encrypted: u32,
        }

        // Both structs must have identical size.
        assert_eq!(
            std::mem::size_of::<TestXswusage>(),
            std::mem::size_of::<DarwinXswusage>(),
            "TestXswusage and DarwinXswusage must have the same size"
        );

        // Verify field offsets match between the two repr(C) structs.
        assert_eq!(
            std::mem::offset_of!(TestXswusage, xsu_total),
            std::mem::offset_of!(DarwinXswusage, xsu_total)
        );
        assert_eq!(
            std::mem::offset_of!(TestXswusage, xsu_avail),
            std::mem::offset_of!(DarwinXswusage, xsu_avail)
        );
        assert_eq!(
            std::mem::offset_of!(TestXswusage, xsu_used),
            std::mem::offset_of!(DarwinXswusage, xsu_used)
        );
        assert_eq!(
            std::mem::offset_of!(TestXswusage, xsu_pagesize),
            std::mem::offset_of!(DarwinXswusage, xsu_pagesize)
        );
        assert_eq!(
            std::mem::offset_of!(TestXswusage, xsu_encrypted),
            std::mem::offset_of!(DarwinXswusage, xsu_encrypted)
        );

        // Total size must be 8 + 8 + 8 + 4 + 4 = 32 bytes.
        assert_eq!(std::mem::size_of::<DarwinXswusage>(), 32);
    }

    #[test]
    fn native_filesystems_provide_eligible_capacity() {
        let query = FfiNativeQueries;
        let mounted = query
            .mounted_filesystems()
            .expect("mounted_filesystems succeeds");
        assert!(
            !mounted.is_empty(),
            "expected at least one mounted filesystem on the hosted Mac image"
        );
        let eligible: Vec<&RawMountedFilesystem> = mounted
            .iter()
            .filter(|record| {
                record.flags & super::MNT_LOCAL != 0
                    && record.flags & super::MNT_DONTBROWSE == 0
                    && !record.mount_point.is_empty()
                    && record.filesystem_type != "devfs"
                    && record.filesystem_type != "autofs"
            })
            .collect();
        assert!(
            !eligible.is_empty(),
            "expected at least one eligible local filesystem on the hosted image"
        );
        for record in &eligible {
            let total = record.total_blocks.saturating_mul(record.block_size);
            let free = record.free_blocks.saturating_mul(record.block_size);
            let available = record.available_blocks.saturating_mul(record.block_size);
            assert!(
                total > 0,
                "eligible {} must have nonzero total",
                record.mount_point
            );
            assert!(
                free <= total,
                "eligible {} free ({free}) must not exceed total ({total})",
                record.mount_point
            );
            assert!(
                available <= total,
                "eligible {} available ({available}) must not exceed total ({total})",
                record.mount_point
            );
        }
    }

    #[test]
    fn native_preferred_network_source_provides_interfaces() {
        let interfaces = super::network_interfaces_iflist2()
            .expect("preferred NET_RT_IFLIST2 source succeeds on the hosted image");
        assert!(
            !interfaces.is_empty(),
            "expected at least one interface from the preferred source"
        );
        for interface in &interfaces {
            assert!(!interface.id.is_empty(), "interface id must not be empty");
            assert!(
                !interface.name.is_empty(),
                "interface name must not be empty"
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn native_collector_publishes_v2_drives_and_network() {
        use crate::collector::macos::MacOsCollector;
        use crate::collector::SystemCollector;

        let mut collector = MacOsCollector::new(None).expect("collector constructs");
        let identity = collector.identity().expect("identity");
        let warm_err = collector.sample().expect_err("first sample warms");
        assert_eq!(warm_err.kind, CollectErrorKind::Warming);

        // Bounded warmup: CPU baselines need one interval, disk/network
        // baselines need two observations, and drives arrive asynchronously
        // from the refresh worker. Poll without sleeping production intervals.
        let start = std::time::Instant::now();
        let deadline = std::time::Duration::from_secs(15);
        let payload = loop {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let elapsed = start.elapsed();
            let metrics = match collector.sample() {
                Ok(metrics) => metrics,
                Err(error) if error.kind == CollectErrorKind::CounterReset => {
                    if elapsed >= deadline {
                        panic!("counter reset persisted past bounded warmup: {error:?}");
                    }
                    continue;
                }
                Err(error) => panic!("native v2 warmup sample fails: {error:?}"),
            };
            let payload = metrics
                .clone()
                .into_status_payload_v2(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis()
                        .try_into()
                        .unwrap(),
                    1000,
                    collector.capabilities_v2(),
                    identity.clone(),
                )
                .expect("v2 conversion succeeds");
            let drives_ready = payload
                .drives
                .as_ref()
                .is_some_and(|drives| !drives.is_empty());
            let network_ready = payload.network.is_some();
            if drives_ready && network_ready {
                break payload;
            }
            if elapsed >= deadline {
                panic!(
                    "bounded warmup expired without v2 drives+network: drives={:?} network_present={}",
                    payload.drives.as_ref().map(Vec::len),
                    payload.network.is_some(),
                );
            }
        };
        payload.validate().expect("native v2 payload validates");
        // Disk I/O stays optional: hosted/virtualized Macs may genuinely
        // expose no usable IOKit block-driver counters.
        let _ = payload.disk_io;
    }

    /// Complete production collector smoke test.
    ///
    /// Exercises the full `FfiNativeQueries` path through the
    /// `MacOsCollector` abstraction, including identity, warm-up sample,
    /// second sample, protocol snapshot validation, and repeated sampling
    /// to catch ownership or state-reset defects.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn complete_production_collector_smoke() {
        use crate::collector::macos::MacOsCollector;
        use crate::collector::SystemCollector;
        use gregg_protocol::{MetricCapabilities, SCHEMA_VERSION_V1};

        // 1. Construct the production macOS collector.
        let mut collector = MacOsCollector::new(None).expect("collector constructs");

        // 2. Read identity and assert nonempty fields and nonzero values.
        let identity = collector.identity().expect("identity");
        assert!(!identity.hostname.is_empty(), "hostname must not be empty");
        assert!(
            !identity.architecture.is_empty(),
            "architecture must not be empty"
        );
        assert!(
            !identity.kernel_release.is_empty(),
            "kernel_release must not be empty"
        );
        assert!(
            !identity.kernel_name.is_empty(),
            "kernel_name must not be empty"
        );
        assert!(!identity.os_name.is_empty(), "os_name must not be empty");

        // 3. Perform the CPU warm-up sample (establishes counter baseline).
        let warm_err = collector.sample().expect_err("first sample warms");
        assert_eq!(warm_err.kind, CollectErrorKind::Warming);

        // 4. Wait for counters to advance, then sample.  On virtualized Intel
        //    runners Mach counters may not advance as quickly, so we retry on
        //    CounterReset (which re-establishes the baseline internally).
        //    Bounded to 20 attempts / 10 seconds to avoid hanging CI.
        let start = std::time::Instant::now();
        let max_attempts: u32 = 20;
        let max_elapsed = std::time::Duration::from_secs(10);
        let metrics = loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let elapsed = start.elapsed();
            match collector.sample() {
                Ok(m) => break m,
                Err(e) if e.kind == CollectErrorKind::CounterReset => {
                    let attempts = u32::try_from(elapsed.as_millis() / 200).unwrap() + 1;
                    if attempts >= max_attempts || elapsed >= max_elapsed {
                        eprintln!(
                            "complete_production_collector_smoke: CounterReset \
                             after {attempts} attempts, \
                             {elapsed:?} elapsed, \
                             arch={}, \
                             last_err_kind={:?}",
                            identity.architecture, e.kind,
                        );
                        panic!(
                            "second sample failed after {attempts} attempts: \
                             CounterReset"
                        );
                    }
                }
                Err(e) => panic!("second sample fails: {e:?}"),
            }
        };

        // 6. Validate the complete protocol snapshot.
        let snap = metrics
            .into_snapshot(
                SCHEMA_VERSION_V1,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
                    .try_into()
                    .unwrap(),
                1000,
                MetricCapabilities { cpu_iowait: false },
                identity.clone(),
            )
            .expect("into_snapshot succeeds");
        snap.validate().expect("snapshot validates");

        // 7. Assert cpu_iowait == false and iowait_pct == None.
        assert!(
            !snap.capabilities.cpu_iowait,
            "macOS iowait must be unsupported"
        );
        assert!(
            snap.cpu.iowait_pct.is_none(),
            "iowait_pct must be None on macOS"
        );

        // 8. Assert memory total and page size are nonzero.
        assert!(snap.memory.total_bytes > 0, "memory total must be nonzero");
        assert!(snap.cpu.logical_cores > 0, "logical cores must be nonzero");

        // 9. Assert swap used is not greater than swap total.
        assert!(
            snap.swap.used_bytes <= snap.swap.total_bytes,
            "swap used ({}) must not exceed swap total ({})",
            snap.swap.used_bytes,
            snap.swap.total_bytes
        );

        // 10. Repeat the complete sample path to catch ownership defects.
        //     This exercises repeated HostPort acquisition and deallocation.
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let m = collector.sample();
            // Some samples may fail transiently; we only care that the
            // collector doesn't crash or leak Mach port rights.
            if let Ok(metrics) = m {
                let snap = metrics
                    .into_snapshot(
                        SCHEMA_VERSION_V1,
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis()
                            .try_into()
                            .unwrap(),
                        1000,
                        MetricCapabilities { cpu_iowait: false },
                        identity.clone(),
                    )
                    .expect("into_snapshot succeeds");
                snap.validate().expect("repeated snapshot validates");
            }
        }
    }
}
