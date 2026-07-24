//! CLI argument parsing and subcommand dispatch for `gregg`.
//!
//! Uses `clap` derive macros for structured argument parsing. Each
//! subcommand has a stable help message and returns a meaningful exit code.

use std::fmt;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config::{Config, ConfigError, ConfigStore};
use crate::endpoint::{EndpointError, EndpointSpec};

/// Compact keyboard-first terminal monitor for multiple remote systems.
#[derive(Parser)]
#[command(
    name = "gregg",
    version,
    about = "Compact terminal monitor for remote system metrics",
    long_about = "gregg polls configured greggd endpoints and renders each system \
                  in four terminal rows. Without a subcommand, it starts the TUI. \
                  Subcommands manage the persistent endpoint configuration."
)]
pub struct Cli {
    /// Path to the configuration file.
    #[arg(
        long,
        short = 'c',
        global = true,
        help = "Path to the TOML configuration file",
        value_name = "PATH"
    )]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Available subcommands.
#[derive(Subcommand)]
pub enum Command {
    /// Add a monitored endpoint.
    ///
    /// Parses the endpoint, assigns a stable UUID, and appends it to the
    /// configuration. Exact duplicates are rejected unless `--replace` is set.
    ///
    /// # Examples
    ///
    /// ```text
    /// gregg add 192.168.1.8
    /// gregg add macmini.local:11310 --name "Mac Mini"
    /// gregg add 10.0.0.5:8080 --replace
    /// ```
    Add {
        /// Endpoint in host:port format (default port 11310).
        endpoint: String,
        /// Optional display name for this endpoint.
        #[arg(long)]
        name: Option<String>,
        /// Replace an existing endpoint with the same host:port.
        #[arg(long)]
        replace: bool,
    },
    /// List all configured endpoints.
    ///
    /// Prints one endpoint per line in stable insertion order. With `--json`,
    /// emits a machine-readable JSON array.
    ///
    /// # Examples
    ///
    /// ```text
    /// gregg list
    /// gregg list --json
    /// ```
    List {
        /// Output in JSON format.
        #[arg(long)]
        json: bool,
    },
    /// Remove one or more monitored endpoints.
    ///
    /// Use host only to remove all entries for that host (regardless of port),
    /// or host:port to remove a specific endpoint.
    ///
    /// # Examples
    ///
    /// ```text
    /// gregg remove 192.168.1.8
    /// gregg remove 10.0.0.5:8080
    /// ```
    Remove {
        /// Endpoint to remove. Use host only to remove all entries for that host,
        /// or host:port to remove a specific endpoint.
        endpoint: String,
    },
    /// Set the global polling interval in seconds.
    ///
    /// Persists the interval to the configuration file. Does not trigger an
    /// immediate poll. Valid range is 1..=3600.
    ///
    /// # Examples
    ///
    /// ```text
    /// gregg refresh 5
    /// gregg refresh 30
    /// ```
    Refresh {
        /// Refresh interval in seconds (1-3600).
        seconds: u64,
    },
    /// Open the configuration file in an editor.
    ///
    /// Resolves the editor from `$VISUAL`, `$EDITOR`, then fallbacks (`hx`,
    /// `vim`, `vi`). Validates the file after the editor exits.
    ///
    /// # Examples
    ///
    /// ```text
    /// gregg edit
    /// gregg --config /tmp/test.toml edit
    /// ```
    Edit,
}

/// Exit codes returned by gregg commands.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ExitCode {
    Success = 0,
    /// Configuration error (invalid, missing, or unwritable).
    ConfigError = 1,
    /// Endpoint parse or validation error.
    EndpointError = 2,
    /// The requested operation could not be completed.
    OperationError = 3,
    /// The config file was not found.
    NotFound = 4,
    /// Editor could not be launched.
    EditorError = 5,
}

impl From<&ConfigError> for ExitCode {
    fn from(e: &ConfigError) -> Self {
        match e {
            ConfigError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                Self::NotFound
            }
            ConfigError::Io { .. } | ConfigError::Parse { .. } | ConfigError::Validation(_) => {
                Self::ConfigError
            }
            ConfigError::AtomicWrite { .. } => Self::ConfigError,
            ConfigError::LockPoisoned => Self::OperationError,
        }
    }
}

impl From<&EndpointError> for ExitCode {
    fn from(_: &EndpointError) -> Self {
        Self::EndpointError
    }
}

/// Resolve the config path: explicit `--config` or platform default.
#[must_use]
pub fn resolve_config_path(explicit: Option<&PathBuf>) -> PathBuf {
    explicit.cloned().unwrap_or_else(Config::default_path)
}

/// Dispatch a subcommand.
///
/// # Errors
///
/// Returns a boxed error if the command fails.
pub fn dispatch(command: &Command, store: &ConfigStore) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Add {
            endpoint,
            name,
            replace,
        } => cmd_add(store, endpoint, name.as_deref(), *replace),
        Command::List { json } => cmd_list(store, *json),
        Command::Remove { endpoint } => cmd_remove(store, endpoint),
        Command::Refresh { seconds } => cmd_refresh(store, *seconds),
        Command::Edit => cmd_edit(store),
    }
}

fn cmd_add(
    store: &ConfigStore,
    endpoint_str: &str,
    name: Option<&str>,
    replace: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Validate name early.
    if let Some(n) = name {
        crate::endpoint::validate_name(n)?;
    }

    let spec = EndpointSpec::parse(endpoint_str)?;

    let result = store.mutate_with_result(|config| {
        let port = if spec.port_was_explicit {
            spec.port
        } else {
            config.default_port
        };

        // Check for exact duplicate.
        let existing_idx = config
            .systems
            .iter()
            .position(|s| s.host == spec.host && s.port == port);

        if let Some(idx) = existing_idx {
            if replace {
                config.systems.remove(idx);
            } else {
                return Err(ConfigError::Validation(vec![
                    crate::config::ConfigViolation::DuplicateAddress {
                        address: crate::endpoint::display_address(&spec.host, port),
                    },
                ]));
            }
        }

        let port_was_explicit = spec.port_was_explicit;
        let ep = spec.into_endpoint();
        let entry = crate::config::SystemEntry {
            id: ep.id,
            host: ep.host,
            port: ep.port,
            port_was_explicit,
            name: name.map(std::string::ToString::to_string),
        };
        config.systems.push(entry);

        Ok(())
    });

    match result {
        Ok(()) => {
            eprintln!("added endpoint {endpoint_str}");
            Ok(())
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn cmd_list(store: &ConfigStore, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let config = store.load_or_default()?;

    if json {
        let output =
            serde_json::to_string_pretty(&config.systems).expect("systems serializes to JSON");
        println!("{output}");
    } else {
        if config.systems.is_empty() {
            // Print nothing for empty list.
            return Ok(());
        }
        for system in &config.systems {
            let ep = system.to_endpoint();
            println!("{ep}");
        }
    }

    Ok(())
}

fn cmd_remove(store: &ConfigStore, endpoint_str: &str) -> Result<(), Box<dyn std::error::Error>> {
    let spec = EndpointSpec::parse(endpoint_str)?;
    let exact_port = if spec.port_was_explicit {
        Some(spec.port)
    } else {
        None // Host-only removal
    };

    let result = store.mutate_with_result(|config| {
        let original_len = config.systems.len();

        if let Some(port) = exact_port {
            // Exact endpoint removal.
            config
                .systems
                .retain(|s| !(s.host == spec.host && s.port == port));
        } else {
            // Host-wide removal.
            config.systems.retain(|s| s.host != spec.host);
        }

        let removed = original_len - config.systems.len();
        Ok(removed)
    });

    match result {
        Ok(removed) => {
            if removed == 0 {
                eprintln!("no matching endpoint found: {endpoint_str}");
            } else {
                eprintln!("removed {removed} endpoint(s)");
            }
            Ok(())
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn cmd_refresh(store: &ConfigStore, seconds: u64) -> Result<(), Box<dyn std::error::Error>> {
    store.mutate(|config| {
        config.refresh_seconds = seconds;
        Ok(())
    })?;
    eprintln!("refresh interval set to {seconds}s");
    Ok(())
}

fn cmd_edit(store: &ConfigStore) -> Result<(), Box<dyn std::error::Error>> {
    // Ensure a config file exists.
    let path = store.path().to_path_buf();
    if !path.exists() {
        let default = Config::default();
        store.write(&default)?;
    }

    // Resolve editor.
    let editor = resolve_editor().ok_or_else(|| {
        Box::new(ConfigError::Io {
            path: path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no editor found; set $VISUAL or $EDITOR",
            ),
        }) as Box<dyn std::error::Error>
    })?;

    // Launch editor.
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .map_err(|e| {
            Box::new(ConfigError::Io {
                path: path.clone(),
                source: e,
            }) as Box<dyn std::error::Error>
        })?;

    if !status.success() {
        return Err(Box::new(ConfigError::Io {
            path: path.clone(),
            source: std::io::Error::other(format!("editor exited with status: {status}")),
        }) as Box<dyn std::error::Error>);
    }

    // Reload and validate.
    match Config::load(&path) {
        Ok(_) => {
            eprintln!("configuration validated successfully");
            Ok(())
        }
        Err(e) => {
            eprintln!("warning: edited config is invalid: {e}");
            Err(Box::new(e))
        }
    }
}

/// Resolve the editor to use, checking $VISUAL, $EDITOR, then fallbacks.
#[must_use]
pub fn resolve_editor() -> Option<String> {
    if let Ok(visual) = std::env::var("VISUAL") {
        let trimmed = visual.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    if let Ok(editor) = std::env::var("EDITOR") {
        let trimmed = editor.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    // Check fallbacks.
    for fallback in &["hx", "vim", "vi"] {
        if which_exists(fallback) {
            return Some((*fallback).to_string());
        }
    }
    None
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Error type wrapping config and endpoint errors.
#[derive(Debug)]
#[allow(dead_code)]
pub enum ClientError {
    Config(ConfigError),
    Endpoint(EndpointError),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "{e}"),
            Self::Endpoint(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Endpoint(e) => Some(e),
        }
    }
}

impl From<ConfigError> for ClientError {
    fn from(e: ConfigError) -> Self {
        Self::Config(e)
    }
}

impl From<EndpointError> for ClientError {
    fn from(e: EndpointError) -> Self {
        Self::Endpoint(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gregg_cli_test_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- CLI parsing ---

    #[test]
    fn cli_parses_no_command() {
        let cli = Cli::try_parse_from(["gregg"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_parses_add() {
        let cli = Cli::try_parse_from(["gregg", "add", "192.168.1.1"]).unwrap();
        match cli.command.unwrap() {
            Command::Add {
                endpoint,
                name,
                replace,
            } => {
                assert_eq!(endpoint, "192.168.1.1");
                assert!(name.is_none());
                assert!(!replace);
            }
            _ => panic!("expected Add command"),
        }
    }

    #[test]
    fn cli_parses_add_with_name() {
        let cli = Cli::try_parse_from(["gregg", "add", "192.168.1.1", "--name", "Server"]).unwrap();
        match cli.command.unwrap() {
            Command::Add { endpoint, name, .. } => {
                assert_eq!(endpoint, "192.168.1.1");
                assert_eq!(name.as_deref(), Some("Server"));
            }
            _ => panic!("expected Add command"),
        }
    }

    #[test]
    fn cli_parses_add_with_replace() {
        let cli = Cli::try_parse_from(["gregg", "add", "192.168.1.1", "--replace"]).unwrap();
        match cli.command.unwrap() {
            Command::Add { replace, .. } => {
                assert!(replace);
            }
            _ => panic!("expected Add command"),
        }
    }

    #[test]
    fn cli_parses_list() {
        let cli = Cli::try_parse_from(["gregg", "list"]).unwrap();
        assert!(matches!(
            cli.command.unwrap(),
            Command::List { json: false }
        ));
    }

    #[test]
    fn cli_parses_list_json() {
        let cli = Cli::try_parse_from(["gregg", "list", "--json"]).unwrap();
        assert!(matches!(cli.command.unwrap(), Command::List { json: true }));
    }

    #[test]
    fn cli_parses_remove() {
        let cli = Cli::try_parse_from(["gregg", "remove", "192.168.1.1"]).unwrap();
        match cli.command.unwrap() {
            Command::Remove { endpoint } => {
                assert_eq!(endpoint, "192.168.1.1");
            }
            _ => panic!("expected Remove command"),
        }
    }

    #[test]
    fn cli_parses_refresh() {
        let cli = Cli::try_parse_from(["gregg", "refresh", "30"]).unwrap();
        match cli.command.unwrap() {
            Command::Refresh { seconds } => {
                assert_eq!(seconds, 30);
            }
            _ => panic!("expected Refresh command"),
        }
    }

    #[test]
    fn cli_parses_edit() {
        let cli = Cli::try_parse_from(["gregg", "edit"]).unwrap();
        assert!(matches!(cli.command.unwrap(), Command::Edit));
    }

    #[test]
    fn cli_parses_config_flag() {
        let cli = Cli::try_parse_from(["gregg", "--config", "/tmp/test.toml", "list"]).unwrap();
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/test.toml")));
    }

    // --- Add command ---

    #[test]
    fn add_first_endpoint() {
        let dir = tmp_dir("add_first");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1", None, false).unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems.len(), 1);
        assert_eq!(config.systems[0].host, "192.168.1.1");
        assert_eq!(config.systems[0].port, 11310);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_named_endpoint() {
        let dir = tmp_dir("add_named");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1:8080", Some("My Server"), false).unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems.len(), 1);
        assert_eq!(config.systems[0].host, "192.168.1.1");
        assert_eq!(config.systems[0].port, 8080);
        assert_eq!(config.systems[0].name.as_deref(), Some("My Server"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_duplicate_rejects() {
        let dir = tmp_dir("add_dup");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1", None, false).unwrap();
        let result = cmd_add(&store, "192.168.1.1", None, false);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_replace_overwrites() {
        let dir = tmp_dir("add_replace");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1", Some("Old"), false).unwrap();
        cmd_add(&store, "192.168.1.1", Some("New"), true).unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems.len(), 1);
        assert_eq!(config.systems[0].name.as_deref(), Some("New"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_without_explicit_port_uses_default_port() {
        let dir = tmp_dir("add_default_port");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        // Add without explicit port — should use default_port.
        cmd_add(&store, "192.168.1.1", None, false).unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems[0].port, 11310);
        assert!(!config.systems[0].port_was_explicit);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_with_explicit_port_stores_flag() {
        let dir = tmp_dir("add_explicit_port");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1:8080", None, false).unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems[0].port, 8080);
        assert!(config.systems[0].port_was_explicit);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_with_default_port_value_explicit_is_marked_explicit() {
        let dir = tmp_dir("add_explicit_default");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        // Explicitly specifying the default port should still be marked explicit.
        cmd_add(&store, "192.168.1.1:11310", None, false).unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems[0].port, 11310);
        assert!(config.systems[0].port_was_explicit);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_without_explicit_port_removes_all_for_host() {
        let dir = tmp_dir("remove_host_all");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1:8080", None, false).unwrap();
        cmd_add(&store, "192.168.1.1:9090", None, false).unwrap();

        // Remove without explicit port — should remove both.
        cmd_remove(&store, "192.168.1.1").unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems.len(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_with_explicit_port_removes_only_exact_match() {
        let dir = tmp_dir("remove_exact_port");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1:8080", None, false).unwrap();
        cmd_add(&store, "192.168.1.1:9090", None, false).unwrap();

        // Remove with explicit port — should only remove 8080.
        cmd_remove(&store, "192.168.1.1:8080").unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems.len(), 1);
        assert_eq!(config.systems[0].port, 9090);

        let _ = fs::remove_dir_all(&dir);
    }

    // --- List command ---

    #[test]
    fn list_empty_config() {
        let dir = tmp_dir("list_empty");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_list(&store, false).unwrap();
        // No output expected.

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_with_endpoints() {
        let dir = tmp_dir("list_endpoints");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1", Some("Server"), false).unwrap();
        cmd_add(&store, "10.0.0.1:8080", None, false).unwrap();

        // Just verify it doesn't panic.
        cmd_list(&store, false).unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_json() {
        let dir = tmp_dir("list_json");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1", None, false).unwrap();

        // Just verify it doesn't panic.
        cmd_list(&store, true).unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Remove command ---

    #[test]
    fn remove_exact_endpoint() {
        let dir = tmp_dir("remove_exact");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1:8080", None, false).unwrap();
        cmd_add(&store, "192.168.1.1:9090", None, false).unwrap();

        cmd_remove(&store, "192.168.1.1:8080").unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems.len(), 1);
        assert_eq!(config.systems[0].port, 9090);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_host_wide() {
        let dir = tmp_dir("remove_host");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1:8080", None, false).unwrap();
        cmd_add(&store, "192.168.1.1:9090", None, false).unwrap();
        cmd_add(&store, "10.0.0.1", None, false).unwrap();

        cmd_remove(&store, "192.168.1.1").unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems.len(), 1);
        assert_eq!(config.systems[0].host, "10.0.0.1");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_nonexistent_is_idempotent() {
        let dir = tmp_dir("remove_none");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        // Should succeed (no error, just a warning).
        cmd_remove(&store, "192.168.1.1").unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Refresh command ---

    #[test]
    fn refresh_sets_interval() {
        let dir = tmp_dir("refresh");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_refresh(&store, 30).unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.refresh_seconds, 30);

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Config path resolution ---

    #[test]
    fn resolve_config_path_explicit() {
        let explicit = PathBuf::from("/custom/path.toml");
        let resolved = resolve_config_path(Some(&explicit));
        assert_eq!(resolved, explicit);
    }

    #[test]
    fn resolve_config_path_default() {
        let resolved = resolve_config_path(None);
        assert_eq!(resolved, Config::default_path());
    }

    // --- Editor resolution ---

    #[test]
    fn resolve_editor_returns_something() {
        // On most systems, at least 'vi' should be available.
        // We just verify the function doesn't panic.
        let _ = resolve_editor();
    }

    // --- Endpoint ordering preserved ---

    #[test]
    fn add_preserves_order() {
        let dir = tmp_dir("order");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1", None, false).unwrap();
        cmd_add(&store, "10.0.0.1", None, false).unwrap();
        cmd_add(&store, "172.16.0.1", None, false).unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.systems[0].host, "192.168.1.1");
        assert_eq!(config.systems[1].host, "10.0.0.1");
        assert_eq!(config.systems[2].host, "172.16.0.1");

        let _ = fs::remove_dir_all(&dir);
    }

    // --- IDs are stable ---

    #[test]
    fn endpoint_ids_are_stable() {
        let dir = tmp_dir("ids");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        cmd_add(&store, "192.168.1.1", None, false).unwrap();

        let config1 = store.load_existing().unwrap();
        let id1 = config1.systems[0].id.clone();

        // Reload and verify ID is stable.
        let config2 = store.load_existing().unwrap();
        assert_eq!(config2.systems[0].id, id1);

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Non-TUI commands never initialize terminal ---

    #[test]
    fn subcommands_dont_panic() {
        let dir = tmp_dir("no_panic");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        // These should all complete without error.
        cmd_add(&store, "192.168.1.1", None, false).unwrap();
        cmd_list(&store, false).unwrap();
        cmd_list(&store, true).unwrap();
        cmd_refresh(&store, 10).unwrap();

        let _ = fs::remove_dir_all(&dir);
    }
}
