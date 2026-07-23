//! `greggd` binary entry point.
//!
//! Parses CLI arguments and dispatches to the appropriate subcommand.
//! The `run` command loads validated config and enters the foreground
//! daemon loop. Lifecycle commands delegate to the native service manager.

use clap::Parser;

#[cfg(target_os = "linux")]
type NativeCollector = greggd::collector::linux::LinuxCollector;

#[cfg(target_os = "macos")]
type NativeCollector = greggd::collector::macos::MacOsCollector;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = greggd::cli::Cli::parse();

    let config_path = greggd::cli::resolve_config_path(cli.config.as_ref());

    match cli.command {
        greggd::cli::Command::Run => {
            let config = greggd::cli::load_config(&config_path, cli.config.is_some())?;
            let collector = NativeCollector::new(None)?;
            greggd::run::run(collector, config).await
        }
        command => {
            // Non-run commands are synchronous.
            let service = greggd::service::platform_service_manager();
            greggd::cli::dispatch(&command, &config_path, service.as_ref())
        }
    }
}
