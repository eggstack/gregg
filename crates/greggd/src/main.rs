//! `greggd` binary entry point.
//!
//! Parses CLI arguments and dispatches to the appropriate subcommand.
//! The `run` command loads validated config and enters the foreground
//! daemon loop. The `service` command (Windows only) enters the SCM
//! service entry point. Lifecycle commands delegate to the native
//! service manager.

use clap::Parser;
use std::error::Error;

#[cfg(target_os = "linux")]
type NativeCollector = greggd::collector::linux::LinuxCollector;

#[cfg(target_os = "macos")]
type NativeCollector = greggd::collector::macos::MacOsCollector;

#[cfg(target_os = "windows")]
type NativeCollector = greggd::collector::windows::WindowsCollector;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    init_logging();
    let code = match run_main().await {
        Ok(()) => greggd::cli::ExitCode::Success,
        Err(error) => {
            eprintln!("error: {error}");
            classify_error(error.as_ref())
        }
    };
    std::process::exit(code as i32);
}

async fn run_main() -> Result<(), Box<dyn Error>> {
    let cli = greggd::cli::Cli::parse();

    let config_was_explicit = cli.config.is_some();
    let config_path = greggd::cli::resolve_config_path(cli.config.as_ref());

    match cli.command {
        greggd::cli::Command::Run => {
            let config = greggd::cli::load_config(&config_path, config_was_explicit)?;
            let collector = NativeCollector::new(None)?;
            greggd::run::run(collector, config).await
        }
        #[cfg(target_os = "windows")]
        greggd::cli::Command::Service => greggd::service::windows::run_service(),
        #[cfg(not(target_os = "windows"))]
        greggd::cli::Command::Service => Err("service mode is only available on Windows".into()),
        command => {
            // Non-run commands are synchronous.
            let service = greggd::service::platform_service_manager();
            greggd::cli::dispatch_with_config_intent(
                &command,
                &config_path,
                config_was_explicit,
                service.as_ref(),
            )
        }
    }
}

fn classify_error(error: &(dyn Error + 'static)) -> greggd::cli::ExitCode {
    if let Some(error) = error.downcast_ref::<greggd::config::ConfigError>() {
        return greggd::cli::ExitCode::from(error);
    }
    if let Some(error) = error.downcast_ref::<greggd::cli::ConfigValidationError>() {
        let _ = error;
        return greggd::cli::ExitCode::ConfigError;
    }
    if let Some(error) = error.downcast_ref::<greggd::service::ServiceError>() {
        return greggd::cli::ExitCode::from(error);
    }
    if let Some(greggd::server::error::ServerError::Bind(source)) =
        error.downcast_ref::<greggd::server::error::ServerError>()
    {
        if source.kind() == std::io::ErrorKind::PermissionDenied {
            return greggd::cli::ExitCode::PermissionDenied;
        }
    }
    greggd::cli::ExitCode::RuntimeError
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
