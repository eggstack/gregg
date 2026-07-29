//! Windows SCM service runtime and `WindowsServiceManager`.
//!
//! On Windows, `greggd` can run as a native Windows service managed by
//! the Service Control Manager (SCM). The service entry point is
//! `run_service`, which is invoked when the binary is launched by the
//! SCM with the `service` subcommand.
//!
//! The CLI `start`/`stop`/`restart`/`croncheck` commands use
//! `WindowsServiceManager` to control the service through native APIs.
//!
//! ## Architecture
//!
//! ```text
//! SCM ─── run_service() ──→ run_with_shutdown() ──→ core daemon
//! CLI ─── WindowsServiceManager ──→ native service APIs
//! ```
//!
//! The service state machine is tested through an injectable
//! `ScmAdapter` trait, keeping deterministic unit tests independent
//! from the real SCM.

#[cfg(any(test, target_os = "windows"))]
use std::time::Duration;

#[cfg(any(test, target_os = "windows"))]
use super::{ServiceError, ServiceManager, ServiceState};

/// Service name registered with the SCM.
pub const SERVICE_NAME: &str = "greggd";

/// Display name shown in the Windows Services console.
pub const SERVICE_DISPLAY_NAME: &str = "Gregg Metrics Daemon";

/// Maximum time (ms) to wait for a state transition before reporting timeout.
#[cfg(any(test, target_os = "windows"))]
const STATE_TRANSITION_TIMEOUT_MS: u64 = 30_000;

/// Interval between state-transition polls.
#[cfg(any(test, target_os = "windows"))]
const STATE_POLL_INTERVAL_MS: u64 = 200;

// ── SCM adapter trait (available on Windows and in tests) ──────────────────

/// Adapter trait for SCM interaction. The production implementation wraps
/// `windows-service` FFI calls; test implementations provide deterministic
/// fake behavior.
#[cfg(any(test, target_os = "windows"))]
pub(crate) trait ScmAdapter: Send + Sync {
    /// Query the current service state from the SCM.
    fn query_state(&self) -> Result<ServiceState, ServiceError>;

    /// Request the SCM to start the service.
    fn start_service(&self) -> Result<(), ServiceError>;

    /// Request the SCM to stop the service.
    fn stop_service(&self) -> Result<(), ServiceError>;
}

/// Wait for the service to reach the target state, polling periodically.
///
/// Returns `Ok(())` when the target state is reached, or
/// `ServiceError::Timeout` if `STATE_TRANSITION_TIMEOUT_MS` elapses.
#[cfg(any(test, target_os = "windows"))]
fn wait_for_state(adapter: &dyn ScmAdapter, target: ServiceState) -> Result<(), ServiceError> {
    let deadline = Duration::from_millis(STATE_TRANSITION_TIMEOUT_MS);
    let poll = Duration::from_millis(STATE_POLL_INTERVAL_MS);
    let start = std::time::Instant::now();

    loop {
        let current = adapter.query_state()?;
        if current == target {
            return Ok(());
        }
        if start.elapsed() >= deadline {
            return Err(ServiceError::Timeout {
                waited_ms: STATE_TRANSITION_TIMEOUT_MS,
            });
        }
        std::thread::sleep(poll);
    }
}

// ── Production SCM adapter (Windows only) ─────────────────────────────────

/// Production SCM adapter using the `windows-service` crate.
///
/// # Safety
///
/// All FFI is delegated to the `windows-service` crate, which manages
/// handle lifetimes and error mapping internally.
#[cfg(target_os = "windows")]
pub(crate) struct NativeScmAdapter {
    service_name: String,
}

#[cfg(target_os = "windows")]
impl NativeScmAdapter {
    /// Create a new adapter for the given service name.
    #[must_use]
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }
}

#[cfg(target_os = "windows")]
impl ScmAdapter for NativeScmAdapter {
    fn query_state(&self) -> Result<ServiceState, ServiceError> {
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(|e| ServiceError::StateQueryFailed {
            source: std::io::Error::other(e),
        })?;

        let service = manager
            .open_service(
                &self.service_name,
                windows_service::service::ServiceAccess::QUERY_STATUS,
            )
            .map_err(|e| ServiceError::StateQueryFailed {
                source: std::io::Error::other(e),
            })?;

        let status = service
            .query_status()
            .map_err(|e| ServiceError::StateQueryFailed {
                source: std::io::Error::other(e),
            })?;

        Ok(map_service_state(status.current_state))
    }

    fn start_service(&self) -> Result<(), ServiceError> {
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(|e| ServiceError::ExecFailed {
            command: "ServiceManager::connect".into(),
            source: std::io::Error::other(e),
        })?;

        let service = manager
            .open_service(
                &self.service_name,
                windows_service::service::ServiceAccess::START,
            )
            .map_err(|e| ServiceError::ExecFailed {
                command: format!("open service `{}`", self.service_name),
                source: std::io::Error::other(e),
            })?;

        let args: [&str; 0] = [];
        service.start(&args).map_err(|e| ServiceError::ExecFailed {
            command: format!("start service `{}`", self.service_name),
            source: std::io::Error::other(e),
        })?;

        Ok(())
    }

    fn stop_service(&self) -> Result<(), ServiceError> {
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(|e| ServiceError::ExecFailed {
            command: "ServiceManager::connect".into(),
            source: std::io::Error::other(e),
        })?;

        let service = manager
            .open_service(
                &self.service_name,
                windows_service::service::ServiceAccess::STOP,
            )
            .map_err(|e| ServiceError::ExecFailed {
                command: format!("open service `{}`", self.service_name),
                source: std::io::Error::other(e),
            })?;

        service.stop().map_err(|e| ServiceError::ExecFailed {
            command: format!("stop service `{}`", self.service_name),
            source: std::io::Error::other(e),
        })?;

        Ok(())
    }
}

/// Map `windows-service` `ServiceState` to our `ServiceState`.
#[cfg(target_os = "windows")]
fn map_service_state(state: windows_service::service::ServiceState) -> ServiceState {
    use windows_service::service::ServiceState as WsState;
    match state {
        WsState::StartPending => ServiceState::StartPending,
        WsState::Running => ServiceState::Running,
        WsState::StopPending => ServiceState::StopPending,
        WsState::Stopped => ServiceState::Stopped,
        WsState::PausePending | WsState::Paused | WsState::ContinuePending => ServiceState::Running,
    }
}

// ── SCM service entry point (Windows only) ────────────────────────────────

/// Windows SCM service entry point.
///
/// Called when `greggd.exe` is invoked by the SCM with the `service`
/// internal command. This function:
///
/// 1. Registers a service control handler with the SCM.
/// 2. Reports `START_PENDING`.
/// 3. Loads configuration from the default Windows path.
/// 4. Constructs a Windows collector.
/// 5. Binds the HTTP listener.
/// 6. Reports `RUNNING`.
/// 7. Enters the shared daemon supervision via `run_with_shutdown`.
/// 8. Reports `STOPPED` on exit.
///
/// # Errors
///
/// Returns an error if the service fails to start.
#[cfg(target_os = "windows")]
pub fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::mpsc;
    use windows_service::service::{ServiceControl, ServiceState as WsState};
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    // Channel for SCM control events.
    let (tx, rx) = mpsc::channel::<ServiceControl>();

    let status_handle =
        service_control_handler::register(SERVICE_NAME, move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = tx.send(control);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        })
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    // Report START_PENDING.
    update_status(
        &status_handle,
        WsState::StartPending,
        0,
        Duration::from_secs(5),
    );

    // Load configuration.
    let config_path = crate::config::Config::default_path();
    let config = crate::config::Config::load(&config_path).unwrap_or_else(|e| {
        eprintln!("failed to load config from {}: {e}", config_path.display());
        std::process::exit(crate::cli::ExitCode::ConfigError as i32);
    });

    // Construct collector.
    let collector = crate::collector::windows::WindowsCollector::new(None)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    // Build the core daemon shutdown future from the SCM control receiver.
    let shutdown_future = async move {
        match rx.recv() {
            Ok(ServiceControl::Stop) => "SCM_STOP",
            Ok(ServiceControl::Shutdown) => "SCM_SHUTDOWN",
            _ => "SCM_UNKNOWN",
        }
    };

    // Report RUNNING before entering the daemon loop.
    update_status(&status_handle, WsState::Running, 0, Duration::from_secs(10));

    // Create a tokio runtime for the async daemon supervision.
    let rt =
        tokio::runtime::Runtime::new().map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    // Enter the shared daemon supervision.
    let result = rt.block_on(crate::run::run_with_shutdown(
        collector,
        config,
        shutdown_future,
    ));

    // Report STOPPED.
    let (exit_code, win_state) = match &result {
        Ok(()) => (0, WsState::Stopped),
        Err(e) => {
            eprintln!("service exited with error: {e}");
            (1, WsState::Stopped)
        }
    };

    update_status(&status_handle, win_state, exit_code, Duration::from_secs(5));

    result
}

/// Update the SCM service status.
#[cfg(target_os = "windows")]
fn update_status(
    status_handle: &windows_service::service_control_handler::ServiceStatusHandle,
    state: windows_service::service::ServiceState,
    exit_code: u32,
    wait_hint: Duration,
) {
    use windows_service::service::{
        ServiceControlAccept, ServiceExitCode, ServiceStatus, ServiceType,
    };

    let status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(exit_code),
        checkpoint: 0,
        wait_hint,
        process_id: None,
    };
    let _ = status_handle.set_service_status(status);
}

// ── WindowsServiceManager ─────────────────────────────────────────────────

/// Windows implementation of [`ServiceManager`] using native SCM APIs.
#[cfg(any(test, target_os = "windows"))]
pub struct WindowsServiceManager {
    adapter: Box<dyn ScmAdapter>,
}

#[cfg(any(test, target_os = "windows"))]
impl std::fmt::Debug for WindowsServiceManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsServiceManager")
            .field("service", &SERVICE_NAME)
            .finish()
    }
}

#[cfg(any(test, target_os = "windows"))]
impl WindowsServiceManager {
    /// Create a production manager using the native SCM adapter.
    ///
    /// # Panics
    ///
    /// Panics if called on a non-Windows platform.
    #[must_use]
    pub fn production() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self {
                adapter: Box::new(NativeScmAdapter::new(SERVICE_NAME)),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            panic!("WindowsServiceManager::production() can only be called on Windows")
        }
    }

    /// Create a manager with a custom adapter (for testing).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_adapter(adapter: Box<dyn ScmAdapter>) -> Self {
        Self { adapter }
    }
}

#[cfg(any(test, target_os = "windows"))]
impl ServiceManager for WindowsServiceManager {
    fn start(&self) -> Result<(), ServiceError> {
        let state = self.adapter.query_state()?;

        match state {
            ServiceState::Running | ServiceState::StartPending => {
                // Already running or starting — idempotent.
                Ok(())
            }
            ServiceState::StopPending => {
                // Wait for stop to complete, then start.
                wait_for_state(&*self.adapter, ServiceState::Stopped)?;
                self.adapter.start_service()?;
                wait_for_state(&*self.adapter, ServiceState::Running)
            }
            ServiceState::Stopped | ServiceState::NotInstalled => {
                self.adapter.start_service()?;
                wait_for_state(&*self.adapter, ServiceState::Running)
            }
        }
    }

    fn stop(&self) -> Result<(), ServiceError> {
        let state = self.adapter.query_state()?;

        match state {
            ServiceState::Stopped | ServiceState::NotInstalled => {
                // Already stopped — idempotent.
                Ok(())
            }
            ServiceState::Running | ServiceState::StartPending => {
                self.adapter.stop_service()?;
                wait_for_state(&*self.adapter, ServiceState::Stopped)
            }
            ServiceState::StopPending => {
                // Already stopping — wait for it.
                wait_for_state(&*self.adapter, ServiceState::Stopped)
            }
        }
    }

    fn restart(&self) -> Result<(), ServiceError> {
        self.stop()?;
        self.start()
    }

    fn is_active(&self) -> Result<bool, ServiceError> {
        let state = self.adapter.query_state()?;
        Ok(state.is_active())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Mock SCM adapter for deterministic tests.
    struct MockScmAdapter {
        state: Mutex<ServiceState>,
        start_error: Mutex<Option<ServiceError>>,
        stop_error: Mutex<Option<ServiceError>>,
        query_error: Mutex<Option<ServiceError>>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl MockScmAdapter {
        fn new(initial: ServiceState) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(initial),
                start_error: Mutex::new(None),
                stop_error: Mutex::new(None),
                query_error: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
            })
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ScmAdapter for MockScmAdapter {
        fn query_state(&self) -> Result<ServiceState, ServiceError> {
            self.calls.lock().unwrap().push("query");
            if let Some(err) = self.query_error.lock().unwrap().take() {
                return Err(err);
            }
            Ok(*self.state.lock().unwrap())
        }

        fn start_service(&self) -> Result<(), ServiceError> {
            self.calls.lock().unwrap().push("start");
            if let Some(err) = self.start_error.lock().unwrap().take() {
                return Err(err);
            }
            *self.state.lock().unwrap() = ServiceState::StartPending;
            Ok(())
        }

        fn stop_service(&self) -> Result<(), ServiceError> {
            self.calls.lock().unwrap().push("stop");
            if let Some(err) = self.stop_error.lock().unwrap().take() {
                return Err(err);
            }
            *self.state.lock().unwrap() = ServiceState::StopPending;
            Ok(())
        }
    }

    /// Thin wrapper to make `MockScmAdapter` work with `Box<dyn ScmAdapter>`.
    struct MockScmAdapterWrapper(Arc<MockScmAdapter>);

    impl ScmAdapter for MockScmAdapterWrapper {
        fn query_state(&self) -> Result<ServiceState, ServiceError> {
            self.0.query_state()
        }
        fn start_service(&self) -> Result<(), ServiceError> {
            self.0.start_service()
        }
        fn stop_service(&self) -> Result<(), ServiceError> {
            self.0.stop_service()
        }
    }

    fn manager_with_mock(
        mock: Arc<MockScmAdapter>,
    ) -> (WindowsServiceManager, Arc<MockScmAdapter>) {
        let mgr =
            WindowsServiceManager::with_adapter(Box::new(MockScmAdapterWrapper(Arc::clone(&mock))));
        (mgr, mock)
    }

    // --- State enum tests ---

    #[test]
    fn service_state_is_active() {
        assert!(ServiceState::Running.is_active());
        assert!(ServiceState::StartPending.is_active());
        assert!(!ServiceState::Stopped.is_active());
        assert!(!ServiceState::StopPending.is_active());
        assert!(!ServiceState::NotInstalled.is_active());
    }

    // --- Start tests ---

    #[test]
    fn start_when_running_is_idempotent() {
        let mock = MockScmAdapter::new(ServiceState::Running);
        let (mgr, mock_ref) = manager_with_mock(mock);

        assert!(mgr.start().is_ok());
        assert_eq!(mock_ref.calls(), vec!["query"]);
    }

    #[test]
    fn start_when_start_pending_is_idempotent() {
        let mock = MockScmAdapter::new(ServiceState::StartPending);
        let (mgr, mock_ref) = manager_with_mock(mock);

        assert!(mgr.start().is_ok());
        assert_eq!(mock_ref.calls(), vec!["query"]);
    }

    // --- Stop tests ---

    #[test]
    fn stop_when_stopped_is_idempotent() {
        let mock = MockScmAdapter::new(ServiceState::Stopped);
        let (mgr, mock_ref) = manager_with_mock(mock);

        assert!(mgr.stop().is_ok());
        assert_eq!(mock_ref.calls(), vec!["query"]);
    }

    #[test]
    fn stop_when_not_installed_is_idempotent() {
        let mock = MockScmAdapter::new(ServiceState::NotInstalled);
        let (mgr, mock_ref) = manager_with_mock(mock);

        assert!(mgr.stop().is_ok());
        assert_eq!(mock_ref.calls(), vec!["query"]);
    }

    #[test]
    fn stop_when_stop_pending_waits_then_times_out() {
        let mock = MockScmAdapter::new(ServiceState::StopPending);
        let (mgr, _mock_ref) = manager_with_mock(mock);

        let result = mgr.stop();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ServiceError::Timeout { .. }));
    }

    // --- Restart tests ---

    #[test]
    fn restart_when_stopped_queries_and_starts() {
        let mock = MockScmAdapter::new(ServiceState::Stopped);
        let (mgr, mock_ref) = manager_with_mock(mock);

        // restart() calls stop() (idempotent on Stopped), then start().
        // start() calls start_service() which sets StartPending, then
        // wait_for_state(Running) which will time out in the mock.
        let _ = mgr.restart();
        let calls = mock_ref.calls();
        // stop query + start query + start_service + wait queries
        assert!(calls.contains(&"start"));
    }

    #[test]
    fn restart_when_running_stops_then_attempts_start() {
        let mock = MockScmAdapter::new(ServiceState::Running);
        let (mgr, mock_ref) = manager_with_mock(mock);

        // restart() calls stop() which calls stop_service(), then
        // wait_for_state(Stopped) times out, so start() is never reached.
        let result = mgr.restart();
        let calls = mock_ref.calls();
        assert!(calls.contains(&"stop"));
        // stop timed out, so restart returns error
        assert!(result.is_err());
    }

    // --- is_active tests ---

    #[test]
    fn is_active_running_returns_true() {
        let mock = MockScmAdapter::new(ServiceState::Running);
        let (mgr, _mock_ref) = manager_with_mock(mock);
        assert!(mgr.is_active().unwrap());
    }

    #[test]
    fn is_active_stopped_returns_false() {
        let mock = MockScmAdapter::new(ServiceState::Stopped);
        let (mgr, _mock_ref) = manager_with_mock(mock);
        assert!(!mgr.is_active().unwrap());
    }

    #[test]
    fn is_active_not_installed_returns_false() {
        let mock = MockScmAdapter::new(ServiceState::NotInstalled);
        let (mgr, _mock_ref) = manager_with_mock(mock);
        assert!(!mgr.is_active().unwrap());
    }

    #[test]
    fn is_active_start_pending_returns_true() {
        let mock = MockScmAdapter::new(ServiceState::StartPending);
        let (mgr, _mock_ref) = manager_with_mock(mock);
        assert!(mgr.is_active().unwrap());
    }

    #[test]
    fn is_active_stop_pending_returns_false() {
        let mock = MockScmAdapter::new(ServiceState::StopPending);
        let (mgr, _mock_ref) = manager_with_mock(mock);
        assert!(!mgr.is_active().unwrap());
    }

    // --- Error propagation tests ---

    #[test]
    fn start_propagates_query_error() {
        let mock = MockScmAdapter::new(ServiceState::Stopped);
        mock.query_error
            .lock()
            .unwrap()
            .replace(ServiceError::AccessDenied);
        let (mgr, _mock_ref) = manager_with_mock(mock);

        let err = mgr.start().unwrap_err();
        assert!(matches!(err, ServiceError::AccessDenied));
    }

    #[test]
    fn stop_propagates_query_error() {
        let mock = MockScmAdapter::new(ServiceState::Running);
        mock.query_error
            .lock()
            .unwrap()
            .replace(ServiceError::AccessDenied);
        let (mgr, _mock_ref) = manager_with_mock(mock);

        let err = mgr.stop().unwrap_err();
        assert!(matches!(err, ServiceError::AccessDenied));
    }

    #[test]
    fn start_propagates_start_error() {
        let mock = MockScmAdapter::new(ServiceState::Stopped);
        mock.start_error
            .lock()
            .unwrap()
            .replace(ServiceError::AccessDenied);
        let (mgr, _mock_ref) = manager_with_mock(mock);

        let err = mgr.start().unwrap_err();
        assert!(matches!(err, ServiceError::AccessDenied));
    }

    #[test]
    fn stop_propagates_stop_error() {
        let mock = MockScmAdapter::new(ServiceState::Running);
        mock.stop_error
            .lock()
            .unwrap()
            .replace(ServiceError::AccessDenied);
        let (mgr, _mock_ref) = manager_with_mock(mock);

        let err = mgr.stop().unwrap_err();
        assert!(matches!(err, ServiceError::AccessDenied));
    }

    // --- Service identity tests ---

    #[test]
    fn service_name_is_stable() {
        assert_eq!(SERVICE_NAME, "greggd");
    }

    #[test]
    fn service_display_name_is_human_readable() {
        assert!(!SERVICE_DISPLAY_NAME.is_empty());
        assert!(!SERVICE_DISPLAY_NAME.contains('\n'));
    }

    // --- Debug test ---

    #[test]
    fn windows_service_manager_debug() {
        let mock = MockScmAdapter::new(ServiceState::Running);
        let (mgr, _mock_ref) = manager_with_mock(mock);
        let debug = format!("{mgr:?}");
        assert!(debug.contains("WindowsServiceManager"));
    }

    // --- ServiceError display tests ---

    #[test]
    fn service_error_access_denied_display() {
        let err = ServiceError::AccessDenied;
        let msg = format!("{err}");
        assert!(msg.contains("access denied"));
    }

    #[test]
    fn service_error_timeout_display() {
        let err = ServiceError::Timeout { waited_ms: 5000 };
        let msg = format!("{err}");
        assert!(msg.contains("5000"));
        assert!(msg.contains("timed out"));
    }

    // --- Exit code tests ---

    #[test]
    fn access_denied_maps_to_permission_denied() {
        let code = crate::cli::ExitCode::from(&ServiceError::AccessDenied);
        assert_eq!(code, crate::cli::ExitCode::PermissionDenied);
    }

    #[test]
    fn timeout_maps_to_service_error() {
        let code = crate::cli::ExitCode::from(&ServiceError::Timeout { waited_ms: 1000 });
        assert_eq!(code, crate::cli::ExitCode::ServiceError);
    }
}
