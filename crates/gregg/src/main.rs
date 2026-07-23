mod cli;
mod config;
mod endpoint;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();

    let config_path = cli::resolve_config_path(cli.config.as_ref());
    let store = config::ConfigStore::new(config_path);

    match &cli.command {
        None => {
            eprintln!("TUI not yet implemented. Use a subcommand.");
            std::process::exit(0);
        }
        Some(command) => {
            if let Err(e) = cli::dispatch(command, &store) {
                eprintln!("error: {e}");
                let code = if let Some(ce) = e.downcast_ref::<config::ConfigError>() {
                    cli::ExitCode::from(ce)
                } else if let Some(ee) = e.downcast_ref::<endpoint::EndpointError>() {
                    cli::ExitCode::from(ee)
                } else {
                    cli::ExitCode::OperationError
                };
                std::process::exit(code as i32);
            }
        }
    }
}
