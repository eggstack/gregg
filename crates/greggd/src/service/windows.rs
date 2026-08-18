//! Windows SCM service runtime and `WindowsServiceManager`.
//!
//! On Windows, `greggd` can run as a native Windows service managed by
//! the Service Control Manager (SCM). The service entry point is
//! `run_service`, which is invoked when the binary is launched by the
//! SCM with the `service` subcommand.
//!
//! The executable's hidden `service` command first enters the SCM dispatcher,
//! which invokes the generated `ServiceMain` callback. The callback runs the
//! service worker and keeps its selected config path in a process-local launch
//! context.
//!
//! The CLI `start`/`stop`/`restart` commands use `WindowsServiceManager` to
//! control the service through native APIs. `croncheck` is a
//! process-local watchdog that probes the TCP listener and spawns `run`
//! directly; it does not interact with the SCM.
//!
//! ## Architecture
//!
//! ```text
//! SCM ─── service_dispatcher ──→ ServiceMain ──→ run_with_shutdown() ──→ core daemon
//! CLI ─── WindowsServiceManager ──→ native service APIs
//! ```
//!
//! The service state machine is tested through an injectable
//! `ScmAdapter` trait, keeping deterministic unit tests independent
//! from the real SCM.

#[cfg(any(test, target_os = "windows"))]
use std::sync::{Arc, Mutex};

#[cfg(any(test, target_os = "windows"))]
use std::time::Duration;

#[cfg(any(test, target_os = "windows"))]
use tokio::sync::oneshot;

#[cfg(any(test, target_os = "windows"))]
use super::{ServiceError, ServiceManager, ServiceState};

#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use std::sync::OnceLock;

#[cfg(target_os = "windows")]
use windows_service::{define_windows_service, service_dispatcher};

/// Service name registered with the SCM.
pub const SERVICE_NAME: &str = "greggd";

/// Display name shown in the Windows Services console.
pub const SERVICE_DISPLAY_NAME: &str = "Gregg Metrics Daemon";

#[cfg(target_os = "windows")]
static SERVICE_LAUNCH_CONFIG: OnceLock<PathBuf> = OnceLock::new();

#[cfg(target_os = "windows")]
define_windows_service!(ffi_service_main, service_main);

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
        WsState::Stopped => ServiceState::Stopped,
        WsState::StopPending => ServiceState::StopPending,
        WsState::Running | WsState::PausePending | WsState::Paused | WsState::ContinuePending => {
            ServiceState::Running
        }
    }
}

// ── SCM service entry point (Windows only) ────────────────────────────────

#[cfg(any(test, target_os = "windows"))]
type ShutdownSender = Arc<Mutex<Option<oneshot::Sender<&'static str>>>>;

#[cfg(any(test, target_os = "windows"))]
fn shutdown_channel() -> (ShutdownSender, oneshot::Receiver<&'static str>) {
    let (sender, receiver) = oneshot::channel();
    (Arc::new(Mutex::new(Some(sender))), receiver)
}

#[cfg(any(test, target_os = "windows"))]
fn send_shutdown(sender: &ShutdownSender, reason: &'static str) {
    if let Ok(mut sender) = sender.lock() {
        if let Some(sender) = sender.take() {
            let _ = sender.send(reason);
        }
    }
}

/// Connect the process to the SCM dispatcher and wait for its callback.
#[cfg(target_os = "windows")]
pub fn start_service_dispatcher(config_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    SERVICE_LAUNCH_CONFIG.set(config_path).map_err(|_| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "Windows service launch context was already initialized",
        )) as Box<dyn std::error::Error>
    })?;

    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

#[cfg(target_os = "windows")]
fn service_main(_service_arguments: Vec<std::ffi::OsString>) {
    let result = SERVICE_LAUNCH_CONFIG
        .get()
        .ok_or_else(|| {
            Box::new(std::io::Error::other(
                "Windows service launch context is missing",
            )) as Box<dyn std::error::Error>
        })
        .and_then(|config_path| run_service_worker(config_path));

    if let Err(error) = result {
        tracing::error!(error = %error, "Windows service exited with an error");
    }
}

#[cfg(target_os = "windows")]
fn run_service_worker(config_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use windows_service::service::ServiceState as WsState;
    use windows_service::service_control_handler;

    // The handler only sends into this one-shot signal. It never waits for the
    // daemon, so SCM callbacks remain nonblocking.
    let (shutdown_sender, shutdown_receiver) = shutdown_channel();
    let status_handle = service_control_handler::register(SERVICE_NAME, move |control| {
        handle_service_control(control, &shutdown_sender)
    })
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    let result = (|| {
        update_status(
            status_handle,
            WsState::StartPending,
            0,
            Duration::from_secs(5),
        )
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        let config = crate::config::Config::load(config_path)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        let collector =
            crate::collector::windows::WindowsCollector::new(Some(config.name.as_str()))
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        let shutdown_future =
            async move { shutdown_receiver.await.unwrap_or("SCM_CHANNEL_CLOSED") };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        rt.block_on(crate::run::run_with_shutdown_on_ready(
            collector,
            config,
            shutdown_future,
            || {
                update_status(status_handle, WsState::Running, 0, Duration::from_secs(10))
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
            },
        ))
    })();

    let exit_code = u32::from(result.is_err());
    let _ = update_status(
        status_handle,
        WsState::Stopped,
        exit_code,
        Duration::from_secs(5),
    );
    result
}

#[cfg(target_os = "windows")]
fn handle_service_control(
    control: windows_service::service::ServiceControl,
    shutdown: &ShutdownSender,
) -> windows_service::service_control_handler::ServiceControlHandlerResult {
    use windows_service::service::ServiceControl;
    use windows_service::service_control_handler::ServiceControlHandlerResult;

    match control {
        ServiceControl::Stop => {
            send_shutdown(shutdown, "SCM_STOP");
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Shutdown => {
            send_shutdown(shutdown, "SCM_SHUTDOWN");
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    }
}

/// Update the SCM service status.
#[cfg(target_os = "windows")]
fn update_status(
    status_handle: windows_service::service_control_handler::ServiceStatusHandle,
    state: windows_service::service::ServiceState,
    exit_code: u32,
    wait_hint: Duration,
) -> windows_service::Result<()> {
    use windows_service::service::{
        ServiceControlAccept, ServiceExitCode, ServiceStatus, ServiceType,
    };

    let status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(exit_code),
        checkpoint: 0,
        wait_hint,
        process_id: None,
    };
    status_handle.set_service_status(status)
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

    // --- Nonblocking SCM shutdown signal tests ---

    #[tokio::test]
    async fn stop_completes_the_async_shutdown_signal_once() {
        let (sender, receiver) = shutdown_channel();
        send_shutdown(&sender, "SCM_STOP");
        send_shutdown(&sender, "SCM_SHUTDOWN");
        assert_eq!(receiver.await, Ok("SCM_STOP"));
    }

    #[tokio::test]
    async fn shutdown_completes_the_async_shutdown_signal() {
        let (sender, receiver) = shutdown_channel();
        send_shutdown(&sender, "SCM_SHUTDOWN");
        assert_eq!(receiver.await, Ok("SCM_SHUTDOWN"));
    }

    #[tokio::test]
    async fn dropped_shutdown_sender_has_a_stable_reason() {
        let (sender, receiver) = shutdown_channel();
        drop(sender);
        assert_eq!(
            receiver.await.unwrap_or("SCM_CHANNEL_CLOSED"),
            "SCM_CHANNEL_CLOSED"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn stop_control_maps_to_no_error_and_scm_stop() {
        use windows_service::service::ServiceControl;
        use windows_service::service_control_handler::ServiceControlHandlerResult;

        let (sender, mut receiver) = shutdown_channel();
        assert!(matches!(
            handle_service_control(ServiceControl::Stop, &sender),
            ServiceControlHandlerResult::NoError
        ));
        assert_eq!(receiver.try_recv(), Ok("SCM_STOP"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shutdown_control_maps_to_no_error_and_scm_shutdown() {
        use windows_service::service::ServiceControl;
        use windows_service::service_control_handler::ServiceControlHandlerResult;

        let (sender, mut receiver) = shutdown_channel();
        assert!(matches!(
            handle_service_control(ServiceControl::Shutdown, &sender),
            ServiceControlHandlerResult::NoError
        ));
        assert_eq!(receiver.try_recv(), Ok("SCM_SHUTDOWN"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn interrogate_does_not_complete_shutdown() {
        use windows_service::service::ServiceControl;
        use windows_service::service_control_handler::ServiceControlHandlerResult;

        let (sender, mut receiver) = shutdown_channel();
        assert!(matches!(
            handle_service_control(ServiceControl::Interrogate, &sender),
            ServiceControlHandlerResult::NoError
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn unsupported_control_is_not_implemented_and_does_not_shutdown() {
        use windows_service::service::ServiceControl;
        use windows_service::service_control_handler::ServiceControlHandlerResult;

        let (sender, mut receiver) = shutdown_channel();
        assert!(matches!(
            handle_service_control(ServiceControl::Pause, &sender),
            ServiceControlHandlerResult::NotImplemented
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn duplicate_stop_and_shutdown_controls_preserve_first_reason() {
        use windows_service::service::ServiceControl;

        let (sender, mut receiver) = shutdown_channel();
        handle_service_control(ServiceControl::Stop, &sender);
        handle_service_control(ServiceControl::Shutdown, &sender);
        assert_eq!(receiver.try_recv(), Ok("SCM_STOP"));

        let (sender, mut receiver) = shutdown_channel();
        handle_service_control(ServiceControl::Shutdown, &sender);
        handle_service_control(ServiceControl::Stop, &sender);
        assert_eq!(receiver.try_recv(), Ok("SCM_SHUTDOWN"));
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
