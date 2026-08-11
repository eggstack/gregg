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

fn main() {
    init_logging();
    let code = match run_main() {
        Ok(()) => greggd::cli::ExitCode::Success,
        Err(error) => {
            eprintln!("error: {error}");
            classify_error(error.as_ref())
        }
    };
    std::process::exit(code as i32);
}

fn run_main() -> Result<(), Box<dyn Error>> {
    let cli = greggd::cli::Cli::parse();

    let config_was_explicit = cli.config.is_some();
    let config_path = greggd::cli::resolve_config_path(cli.config.as_ref());

    match cli.command {
        greggd::cli::Command::Run => {
            let config = greggd::cli::load_config(&config_path, config_was_explicit)?;
            let collector = NativeCollector::new(Some(config.name.as_str()))?;
            build_runtime()?.block_on(greggd::run::run(collector, config))
        }
        #[cfg(target_os = "windows")]
        greggd::cli::Command::Service => {
            greggd::service::windows::start_service_dispatcher(config_path)
        }
        #[cfg(target_os = "windows")]
        command @ (greggd::cli::Command::Start
        | greggd::cli::Command::Stop
        | greggd::cli::Command::Restart) => {
            let service = greggd::service::platform_service_manager();
            match command {
                greggd::cli::Command::Start => service.start().map_err(Into::into),
                greggd::cli::Command::Stop => service.stop().map_err(Into::into),
                greggd::cli::Command::Restart => service.restart().map_err(Into::into),
                _ => unreachable!(),
            }
        }
        #[cfg(target_os = "windows")]
        command @ (greggd::cli::Command::Host { .. } | greggd::cli::Command::Port { .. }) => {
            greggd::cli::dispatch_with_config_intent(&command, &config_path, config_was_explicit)?;
            greggd::service::platform_service_manager()
                .restart()
                .map_err(Into::into)
        }
        command => {
            // Non-run commands are synchronous.
            greggd::cli::dispatch_with_config_intent(&command, &config_path, config_was_explicit)
        }
    }
}

fn build_runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

#[cfg(test)]
fn command_requires_foreground_runtime(command: &greggd::cli::Command) -> bool {
    matches!(command, greggd::cli::Command::Run)
}

fn classify_error(error: &(dyn Error + 'static)) -> greggd::cli::ExitCode {
    if let Some(error) = error.downcast_ref::<greggd::config::ConfigError>() {
        return greggd::cli::ExitCode::from(error);
    }
    if let Some(error) = error.downcast_ref::<greggd::cli::ConfigValidationError>() {
        let _ = error;
        return greggd::cli::ExitCode::ConfigError;
    }
    #[cfg(target_os = "windows")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn classify_error_preserves_daemon_exit_taxonomy() {
        let config = greggd::config::ConfigError::Io {
            path: PathBuf::from("config.toml"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
        };
        assert_eq!(classify_error(&config), greggd::cli::ExitCode::ConfigError);

        let permission = greggd::server::error::ServerError::Bind(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));
        assert_eq!(
            classify_error(&permission),
            greggd::cli::ExitCode::PermissionDenied
        );

        let runtime = greggd::server::error::ServerError::Runtime(std::io::Error::other("runtime"));
        assert_eq!(
            classify_error(&runtime),
            greggd::cli::ExitCode::RuntimeError
        );

        let validation = greggd::cli::ConfigValidationError(Vec::new());
        assert_eq!(
            classify_error(&validation),
            greggd::cli::ExitCode::ConfigError
        );
        assert_eq!(greggd::cli::ExitCode::Success as i32, 0);
    }

    #[test]
    fn runtime_helper_owns_an_immediately_ready_future() {
        let runtime = build_runtime().expect("current-thread runtime should build");
        assert_eq!(runtime.block_on(async { 42 }), 42);
    }

    #[test]
    fn only_foreground_run_requires_a_runtime_at_dispatch_boundary() {
        assert!(command_requires_foreground_runtime(
            &greggd::cli::Command::Run
        ));
        assert!(!command_requires_foreground_runtime(
            &greggd::cli::Command::Host {
                address: "127.0.0.1".parse().expect("valid test address")
            }
        ));
        // Windows service mode owns its runtime inside the SCM service worker, after
        // this synchronous dispatch boundary has selected the command.
        assert!(!command_requires_foreground_runtime(
            &greggd::cli::Command::Version
        ));
    }
}
