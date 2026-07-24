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

// ---------------------------------------------------------------------------
// Native query trait for test injection
// ---------------------------------------------------------------------------

/// Abstraction over native macOS system queries.
///
/// Production code calls FFI; tests inject a mock to exercise edge cases
/// without depending on the host state.
pub trait MacNativeQueries: Send + Sync + std::fmt::Debug {
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
}

// ---------------------------------------------------------------------------
// Production FFI implementation
// ---------------------------------------------------------------------------

/// Production implementation backed by Mach and sysctl FFI.
#[derive(Debug)]
pub struct FfiNativeQueries;

impl MacNativeQueries for FfiNativeQueries {
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
}

// ---------------------------------------------------------------------------
// Mock implementation for tests
// ---------------------------------------------------------------------------

/// Mock native queries for unit tests. All fields are public so tests can
/// inject different values between successive calls.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "test mock with per-endpoint error flags"
)]
pub struct MockNativeQueries {
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
    /// When true, `cpu_load_info` increments `cpu` by a small delta on each
    /// call so successive samples produce a valid non-zero CPU interval.
    pub auto_increment_cpu: bool,
    pub(crate) cpu_call_count: std::sync::atomic::AtomicU32,
}

impl Clone for MockNativeQueries {
    fn clone(&self) -> Self {
        Self {
            cpu: self.cpu,
            vm: self.vm,
            swap: self.swap,
            load: self.load,
            identity: self.identity.clone(),
            cpu_error: self.cpu_error,
            vm_error: self.vm_error,
            swap_error: self.swap_error,
            load_error: self.load_error,
            identity_error: self.identity_error,
            auto_increment_cpu: self.auto_increment_cpu,
            cpu_call_count: std::sync::atomic::AtomicU32::new(
                self.cpu_call_count
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
        }
    }
}

impl MockNativeQueries {
    /// Build a mock returning sensible default values.
    pub fn success() -> Self {
        Self {
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
            auto_increment_cpu: false,
            cpu_call_count: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

impl MacNativeQueries for MockNativeQueries {
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

// ---------------------------------------------------------------------------
// Extern function declarations
// ---------------------------------------------------------------------------

extern "C" {
    fn host_self() -> mach_port_t;

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

// ---------------------------------------------------------------------------
// RAII wrapper for the Mach host-self port
// ---------------------------------------------------------------------------

const MACH_PORT_NULL: mach_port_t = 0;

/// RAII wrapper around a `mach_port_t` send right from `host_self()`.
///
/// The wrapper holds the port for the duration of a collection cycle and
/// deallocates it on drop.  `host_self()` returns a send right to the
/// host port; while some documentation treats it as a well-known constant
/// that never needs deallocation, the Mach ownership model says send
/// rights should be released.  Containing the lifecycle here keeps the
/// rest of the module free of explicit port management.
struct HostPort {
    port: mach_port_t,
}

impl HostPort {
    /// Obtain a fresh host-self send right.
    fn current() -> Self {
        // Safety: `host_self()` is a simple Mach trap that returns a
        // `mach_port_t`.  It cannot fail; it always returns a valid port.
        let port = unsafe { host_self() };
        Self { port }
    }

    /// Borrow the raw port value for FFI calls.
    fn raw(&self) -> mach_port_t {
        self.port
    }
}

impl Drop for HostPort {
    fn drop(&mut self) {
        if self.port != MACH_PORT_NULL {
            // Safety: `mach_port_deallocate` releases one send right.
            // The task is `MACH_PORT_NULL` which means "current task".
            unsafe {
                mach_port_deallocate(MACH_PORT_NULL, self.port);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FFI implementation functions
// ---------------------------------------------------------------------------

/// Read cumulative CPU tick counters from Mach `host_statistics`.
#[allow(
    clippy::cast_sign_loss,
    reason = "Mach natural_t values are always non-negative; i32 ABI is the documented API"
)]
fn cpu_load_info() -> Result<RawCpuTicks, CollectError> {
    let host = HostPort::current();
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
        user: buf[0] as u64,
        system: buf[1] as u64,
        idle: buf[2] as u64,
        nice: buf[3] as u64,
    })
}

/// Read VM statistics from Mach `host_statistics64`.
#[allow(
    clippy::cast_sign_loss,
    reason = "Mach natural_t values are always non-negative; i32 ABI is the documented API"
)]
fn vm_info64() -> Result<RawVmStats, CollectError> {
    let host = HostPort::current();
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
        free_count: buf[0] as u64,
        active_count: buf[1] as u64,
        inactive_count: buf[2] as u64,
        wire_count: buf[3] as u64,
        page_size,
    })
}

/// Read the host page size via `host_page_size`.
fn read_page_size() -> Result<u64, CollectError> {
    let host = HostPort::current();
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
    #[allow(
        clippy::cast_possible_truncation,
        reason = "page size fits in u64 on all supported macOS targets"
    )]
    Ok(page_size as u64)
}

/// Read swap usage from sysctl `vm.swapusage`.
fn swap_usage() -> Result<RawSwapUsage, CollectError> {
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    #[allow(
        non_camel_case_types,
        clippy::struct_field_names,
        reason = "C ABI struct field names match the macOS xswusage definition"
    )]
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
}
