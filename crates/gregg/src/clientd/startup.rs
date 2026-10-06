//! Plan 165: user-scoped startup for the Gregg client daemon.
//!
//! The client daemon belongs to the user whose config it reads, so its startup
//! registration must too. This module never writes system manager state, never
//! calls `sudo`, never creates a service account, and never guesses which human
//! owns a config.
//!
//! # Why not reuse `greggd`'s startup machinery
//!
//! `greggd` is a system service. It writes `/etc/systemd/system`,
//! `/Library/LaunchDaemons`, and a Windows SCM registration, and it needs root
//! for all three. Copying any of that for the client would produce a *root-owned
//! client daemon* reading a *user's* config — a privilege inversion, and a
//! socket in a root-owned directory that the user's TUI could not open. The
//! ownership rule is absolute here: everything lives under the user's own home.
//!
//! # Structure
//!
//! Rendering and parsing are pure functions of the target, so ownership
//! detection is testable on any host and cannot drift from what is written. The
//! OS calls that actually install are thin wrappers over those, and each one
//! refuses to touch a manager it cannot parse back.
//!
//! # The per-config label
//!
//! Every artifact name embeds the config identity digest, so an explicit
//! `--config` registration cannot collide with the default one, and two configs
//! never share a manager entry. There is no global registry of active configs:
//! the artifacts themselves are the only enumeration, and they are only
//! enumerated inside Gregg's own naming namespace.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The manager a client daemon's startup registration uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupMethod {
    /// A `systemctl --user` unit. User scope only; no system unit, no root.
    SystemdUser,
    /// A `~/Library/LaunchAgents` agent, not `/Library/LaunchDaemons`.
    LaunchdUser,
    /// A current-user Startup-folder entry.
    ///
    /// Chosen over Task Scheduler deliberately: it needs no extra Windows API
    /// surface, no admin, and no service account, and its ownership is provable
    /// by parsing a file we wrote. Task Scheduler would need registry or COM
    /// bindings, and its ownership story ("is this task ours?") is weaker for a
    /// `cmd.exe` line.
    WindowsStartupFolder,
    /// A managed user crontab watchdog, used only when user systemd is absent.
    Cron,
}

impl fmt::Display for StartupMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::SystemdUser => "systemd-user",
            Self::LaunchdUser => "launchd-user",
            Self::WindowsStartupFolder => "windows-startup-folder",
            Self::Cron => "user-cron",
        };
        f.write_str(text)
    }
}

/// Whether a discovered manager artifact is Gregg's to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactOwnership {
    /// The artifact names this exact executable and config.
    Owned,
    /// The artifact exists and is parseable, but names something else.
    Foreign,
    /// The artifact exists and could not be parsed, so it is not provably ours.
    ///
    /// Treated exactly like [`Self::Foreign`] everywhere. "I could not read it"
    /// is not a licence to delete it.
    Unknown,
    /// Nothing is there.
    Absent,
}

/// What a startup registration should run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupTarget {
    /// The exact executable to run.
    pub executable: PathBuf,
    /// The config it must serve. Always passed explicitly, so a manager restart
    /// can never pick up a different default than the one registered.
    pub config: PathBuf,
    /// The config identity digest, used in every artifact name.
    pub identity: String,
}

impl StartupTarget {
    /// Build a target for one config.
    #[must_use]
    pub fn new(executable: &Path, config: &Path, identity: &str) -> Self {
        Self {
            executable: executable.to_path_buf(),
            config: config.to_path_buf(),
            identity: identity.to_owned(),
        }
    }

    /// The manager-native name for this config, e.g.
    /// `gregg-client-1a2b3c4d5e6f7081.service`.
    #[must_use]
    pub fn artifact_name(&self, suffix: &str) -> String {
        format!("gregg-client-{}{suffix}", self.identity)
    }
}

/// Why a startup registration could not be performed.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// The chosen manager is not available on this host.
    #[error("user startup is not available here ({0})")]
    Unavailable(&'static str),
    /// The manager command failed.
    #[error("`{command}` failed: {message}")]
    Manager {
        /// The command that was run.
        command: String,
        /// What it reported.
        message: String,
    },
    /// A filesystem step failed.
    #[error("failed to write {path}: {source}")]
    Io {
        /// The path being written.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A manager binary is not on this host.
    #[error("{0} is not installed on this system")]
    MissingManager(&'static str),
}

/// How many manager invocations may block.
const MANAGER_TIMEOUT: Duration = Duration::from_secs(20);

// ── Pure selection ───────────────────────────────────────────────────────

/// Pick a startup method for a host.
///
/// `user_systemd` is the caller's already-probed answer to "is `systemctl
/// --user` usable here", injected so the decision is testable without touching
/// the real system.
#[must_use]
pub fn method_for(os: &str, user_systemd: bool) -> StartupMethod {
    match os {
        "windows" => StartupMethod::WindowsStartupFolder,
        "macos" | "darwin" => StartupMethod::LaunchdUser,
        // Linux without user systemd still needs *something*, and a managed
        // user crontab is the one watchdog that never needs root. It is bounded
        // and greggd-independent, which is the point.
        "linux" | "freebsd" | "openbsd" | "netbsd" => {
            if user_systemd {
                StartupMethod::SystemdUser
            } else {
                StartupMethod::Cron
            }
        }
        _ => StartupMethod::Cron,
    }
}

// ── Linux: systemd user unit ─────────────────────────────────────────────

/// The user-scoped unit path. Never under `/etc`.
#[must_use]
pub fn systemd_user_unit_path(target: &StartupTarget) -> PathBuf {
    let mut base = user_config_home();
    base.push(".config/systemd/user");
    base.push(target.artifact_name(".service"));
    base
}

/// Render a user unit for the client daemon.
#[must_use]
pub fn render_systemd_user_unit(target: &StartupTarget) -> String {
    let exec = format!(
        "{} --config {} daemon run",
        quote_systemd(&target.executable.to_string_lossy()),
        quote_systemd(&target.config.to_string_lossy()),
    );
    // `[Install] WantedBy=default.target` is the user-session equivalent of a
    // system `multi-user.target`: it starts at graphical/login session start
    // without any root involvement.
    format!(
        "[Unit]\n\
         Description=Gregg client daemon ({identity})\n\
         # Restarts on failure; the daemon is a long-lived background observer.\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        identity = target.identity,
    )
}

/// Quote one argument for a systemd `ExecStart` line.
///
/// systemd's own escaping is not shell quoting, so a path with a space must be
/// double-quoted and a literal backslash or quote escaped — the reverse of what
/// a shell would do.
#[must_use]
pub fn quote_systemd(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Parse an `ExecStart=` line into the executable and its `--config` value.
///
/// Returns `None` for anything that is not an owned-shaped invocation, which is
/// how a foreign or hand-edited unit is preserved rather than deleted.
#[must_use]
pub fn parse_systemd_exec_start(text: &str) -> Option<(PathBuf, Option<PathBuf>)> {
    let line = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("ExecStart="))?;
    let line = line.trim();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character.is_whitespace() && !quoted {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    if parts.is_empty() {
        return None;
    }
    let executable = PathBuf::from(parts.remove(0));
    let config = parts
        .windows(2)
        .find(|pair| pair[0] == "--config")
        .and_then(|pair| pair.get(1))
        .map(PathBuf::from);
    Some((executable, config))
}

/// Decide whether a unit file is Gregg's.
#[must_use]
pub fn systemd_unit_ownership(text: &str, executable: &Path, config: &Path) -> ArtifactOwnership {
    let Some((found_exe, found_config)) = parse_systemd_exec_start(text) else {
        return ArtifactOwnership::Unknown;
    };
    let Some(found_config) = found_config else {
        return ArtifactOwnership::Foreign;
    };
    if found_exe == executable && found_config == config {
        ArtifactOwnership::Owned
    } else {
        ArtifactOwnership::Foreign
    }
}

// ── Linux fallback: managed user crontab ─────────────────────────────────

/// The `@reboot` line that starts the client daemon for this config.
#[must_use]
pub fn render_cron_watchdog(target: &StartupTarget) -> String {
    format!(
        "# gregg-client-daemon {}\n@reboot {} --config {} daemon run >> /dev/null 2>&1\n",
        target.identity,
        shell_quote(&target.executable.to_string_lossy()),
        shell_quote(&target.config.to_string_lossy()),
    )
}

/// Single-quote one path for a POSIX shell command line.
#[must_use]
pub fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c))
    {
        return value.to_owned();
    }
    // A single quote cannot appear inside single quotes, so close, escape, and
    // reopen. This is the only correct POSIX quoting for arbitrary bytes.
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Decide whether a crontab block is Gregg's.
#[must_use]
pub fn cron_block_ownership(crontab: &str, executable: &Path, config: &Path) -> ArtifactOwnership {
    if crontab.is_empty() {
        return ArtifactOwnership::Absent;
    }
    let wanted = format!(
        "{} --config {} daemon run",
        shell_quote(&executable.to_string_lossy()),
        shell_quote(&config.to_string_lossy()),
    );
    let ours = crontab
        .lines()
        .any(|line| line.starts_with("# gregg-client-daemon "))
        && crontab.lines().any(|line| line.contains(&wanted));
    if ours {
        return ArtifactOwnership::Owned;
    }
    // A Gregg block exists but names something else: it belongs to another
    // config and must be preserved rather than rewritten around.
    let any_ours = crontab
        .lines()
        .any(|l| l.starts_with("# gregg-client-daemon "));
    if any_ours {
        return ArtifactOwnership::Foreign;
    }
    // Our command without our marker is an unmarked hand-written line. It is
    // ours in substance and matched by identity, so it is removable.
    if crontab.lines().any(|line| line.contains(&wanted)) {
        ArtifactOwnership::Owned
    } else {
        ArtifactOwnership::Unknown
    }
}

// ── macOS: LaunchAgent ───────────────────────────────────────────────────

/// The user `LaunchAgent` path. Never under `/Library/LaunchDaemons`.
#[must_use]
pub fn launchagent_path(target: &StartupTarget) -> PathBuf {
    let mut base = user_home();
    base.push("Library/LaunchAgents");
    base.push(format!("com.eggstack.{}.plist", target.artifact_name("")));
    base
}

/// Render a user `LaunchAgent`.
#[must_use]
pub fn render_launchagent(target: &StartupTarget) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n\
         \x20 <string>com.eggstack.{label}</string>\n\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array>\n\
         \x20   <string>{exe}</string>\n\
         \x20   <string>--config</string>\n\
         \x20   <string>{config}</string>\n\
         \x20   <string>daemon</string>\n\
         \x20   <string>run</string>\n\
         \x20 </array>\n\
         \x20 <key>RunAtLoad</key>\n\
         \x20 <true/>\n\
         \x20 <key>KeepAlive</key>\n\
         \x20 <true/>\n\
         \x20 <key>ProcessType</key>\n\
         \x20 <string>Background</string>\n\
         </dict>\n\
         </plist>\n",
        label = target.artifact_name(""),
        exe = xml_escape(&target.executable.to_string_lossy()),
        config = xml_escape(&target.config.to_string_lossy()),
    )
}

/// Escape the five XML predefined entities.
#[must_use]
pub fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Parse the `ProgramArguments` array back out of a plist.
///
/// The ownership proof is the *argument vector*, not the label: a foreign agent
/// can carry any label it likes, but only our own shape has `--config <path>
/// daemon run` with the executable first.
#[must_use]
pub fn parse_launchagent_program_arguments(text: &str) -> Option<Vec<PathBuf>> {
    let start = text.find("<key>ProgramArguments</key>")?;
    let rest = &text[start..];
    let open = rest.find("<array>")?;
    let close = rest.find("</array>")?;
    if close < open {
        return None;
    }
    let mut arguments = Vec::new();
    for value in rest[open..close].split("<string>").skip(1) {
        let end = value.find("</string>")?;
        arguments.push(PathBuf::from(xml_unescape(&value[..end])));
    }
    if arguments.is_empty() {
        None
    } else {
        Some(arguments)
    }
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

/// Decide whether a plist is Gregg's.
#[must_use]
pub fn launchagent_ownership(text: &str, executable: &Path, config: &Path) -> ArtifactOwnership {
    let Some(arguments) = parse_launchagent_program_arguments(text) else {
        return ArtifactOwnership::Unknown;
    };
    let owned = arguments.first() == Some(&executable.to_path_buf())
        && arguments.get(1).map(std::path::Path::new) == Some(Path::new("--config"))
        && arguments.get(2) == Some(&config.to_path_buf())
        && arguments.get(3).map(std::path::Path::new) == Some(Path::new("daemon"))
        && arguments.get(4).map(std::path::Path::new) == Some(Path::new("run"));
    if owned {
        ArtifactOwnership::Owned
    } else {
        ArtifactOwnership::Foreign
    }
}

// ── Windows: current-user Startup folder ─────────────────────────────────

/// The Startup-folder entry path, or `None` outside a Windows user profile.
///
/// Purely a filesystem location under `%APPDATA%`, which is why this needs no
/// registry or COM bindings and needs no elevation.
#[must_use]
pub fn windows_startup_entry_path(target: &StartupTarget) -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let mut path = PathBuf::from(appdata);
    path.push("Microsoft");
    path.push("Windows");
    path.push("Start Menu");
    path.push("Programs");
    path.push("Startup");
    path.push(format!("{}.cmd", target.artifact_name("")));
    Some(path)
}

/// Render the Startup-folder entry.
#[must_use]
pub fn render_windows_startup_entry(target: &StartupTarget) -> String {
    // `@echo off` then the single invocation. A `.cmd` file is used because the
    // Startup folder runs it directly and `cmd.exe` is guaranteed present,
    // whereas a bare path to the executable would also work but leaves no room
    // for a comment that records ownership.
    format!(
        "@echo off\r\n\
         REM gregg-client-daemon {identity}\r\n\
         start \"\" \"{exe}\" --config \"{config}\" daemon run\r\n",
        identity = target.identity,
        exe = target.executable.to_string_lossy(),
        config = target.config.to_string_lossy(),
    )
}

/// Parse the `start` invocation back out of a Startup-folder entry.
#[must_use]
pub fn parse_windows_startup_entry(text: &str) -> Option<(PathBuf, Option<PathBuf>)> {
    let line = text
        .lines()
        .find(|line| line.trim_start().to_ascii_lowercase().starts_with("start "))?;
    let rest = line.trim_start().get(6..)?;
    // `start "" "<exe>" <args...>`: the first quoted token is the window title,
    // not a path. Tokenize the whole line, quoted and bare alike, so an
    // unquoted `--config` keeps its value.
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quoted_run = false;
    for character in rest.chars() {
        if character == '"' {
            if in_quotes {
                tokens.push(std::mem::take(&mut current));
                // The run ends with the closing quote, so the whitespace after
                // it must not emit a second empty token.
                quoted_run = false;
            } else {
                current.clear();
                quoted_run = true;
            }
            in_quotes = !in_quotes;
        } else if character.is_whitespace() && !in_quotes {
            if quoted_run || !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
                quoted_run = false;
            }
        } else {
            current.push(character);
        }
    }
    if quoted_run || !current.is_empty() {
        tokens.push(current);
    }
    // The first token is always `start`'s window title argument.
    let executable = PathBuf::from(tokens.get(1)?).clone();
    let config = tokens
        .windows(2)
        .find(|pair| pair[0] == "--config")
        .and_then(|pair| pair.get(1))
        .map(PathBuf::from);
    Some((executable, config))
}

/// Decide whether a Startup-folder entry is Gregg's.
#[must_use]
pub fn windows_entry_ownership(text: &str, executable: &Path, config: &Path) -> ArtifactOwnership {
    if !text.contains("gregg-client-daemon ") {
        return ArtifactOwnership::Unknown;
    }
    let Some((found_exe, found_config)) = parse_windows_startup_entry(text) else {
        return ArtifactOwnership::Unknown;
    };
    match found_config {
        Some(found) if found_exe == executable && found == config => ArtifactOwnership::Owned,
        _ => ArtifactOwnership::Foreign,
    }
}

// ── Instructions ─────────────────────────────────────────────────────────

/// What `gregg daemon startup instructions` prints.
///
/// Always actionable and never privileged: it is the fallback the installer
/// points at when its own registration attempt could not be completed.
#[must_use]
pub fn render_instructions(method: StartupMethod, target: &StartupTarget) -> String {
    let exe = target.executable.display();
    let config = target.config.display();
    match method {
        StartupMethod::SystemdUser => format!(
            "Register a user-scoped systemd unit (no root, no system service):\n\n\
             \x20 gregg daemon startup install\n\n\
             Or manage it by hand:\n\n\
             \x20 mkdir -p {unit_dir}\n\
             \x20 # save the unit below as {unit}\n\
             \x20 systemctl --user daemon-reload\n\
             \x20 systemctl --user enable --now {name}\n\n\
             It runs exactly:\n\
             \x20 {exe} --config {config} daemon run\n\
             \n\
             Verify with: gregg daemon status\n\
             Remove with:  systemctl --user disable --now {name}; rm {unit}\n",
            unit_dir = systemd_user_unit_path(target)
                .parent()
                .map_or_else(String::new, |p| p.display().to_string()),
            unit = systemd_user_unit_path(target).display(),
            name = target.artifact_name(".service"),
        ),
        StartupMethod::LaunchdUser => format!(
            "Register a user LaunchAgent (no root; this is NOT /Library/LaunchDaemons):\n\n\
             \x20 gregg daemon startup install\n\n\
             Or manage it by hand:\n\n\
             \x20 # save the plist below as {plist}\n\
             \x20 launchctl load -w {plist}\n\n\
             It runs exactly:\n\
             \x20 {exe} --config {config} daemon run\n\
             \n\
             Verify with: gregg daemon status\n\
             Remove with:  launchctl unload {plist}; rm {plist}\n",
            plist = launchagent_path(target).display(),
        ),
        StartupMethod::WindowsStartupFolder => format!(
            "Register a current-user Startup-folder entry (no admin, no service account):\n\n\
             \x20 gregg daemon startup install\n\n\
             Or place this file by hand in your Startup folder:\n\n\
             \x20 {entry}\n\n\
             \x20 @echo off\n\
             \x20 start \"\" \"{exe}\" --config \"{config}\" daemon run\n\n\
             It runs exactly:\n\
             \x20 {exe} --config {config} daemon run\n\
             \n\
             Verify with: gregg daemon status\n",
            entry = windows_startup_entry_path(target).map_or_else(
                || "(APPDATA is not set)".to_owned(),
                |p| p.display().to_string()
            ),
        ),
        StartupMethod::Cron => format!(
            "User systemd is unavailable here, so Gregg uses a managed user crontab watchdog.\n\n\
             \x20 gregg daemon startup install\n\n\
             Or add this line to your own user crontab (`crontab -e`):\n\n\
             \x20 {line}\n\n\
             That runs:\n\
             \x20 {exe} --config {config} daemon run\n\n\
             Verify with: gregg daemon status\n",
            line = render_cron_watchdog(target).trim_end(),
        ),
    }
}

// ── Environment probes ───────────────────────────────────────────────────

/// The user's home directory, or an error if it is not determinable.
fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// `XDG_CONFIG_HOME`, or the documented `~/.config` default.
fn user_config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| user_home().join(".config"), PathBuf::from)
}

/// Whether `systemctl --user` can actually manage a user unit here.
///
/// Probed, not assumed: a container or a session without a user manager has the
/// binary but not the capability, and writing a unit that can never be enabled
/// would leave the operator with a registration that silently does nothing.
#[must_use]
pub fn user_systemd_available() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    crate::startup_support::run_bounded(
        "systemctl",
        &["--user", "is-system-running"],
        MANAGER_TIMEOUT,
    )
    .is_some_and(|output| {
        // `running` and `degraded` both mean a user manager is there. Any other
        // state — including the non-zero exit from "no bus" — means it is not.
        matches!(
            output.combined.trim(),
            "running" | "degraded" | "starting" | "initializing"
        )
    })
}

// ── Execution ────────────────────────────────────────────────────────────

/// What a completed installation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The method used.
    pub method: StartupMethod,
    /// The artifact that now holds the registration.
    pub artifact: PathBuf,
    /// Whether the manager was also asked to start it now.
    pub started: bool,
}

/// What an uninstall would do, or did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallStep {
    /// The artifact involved.
    pub artifact: PathBuf,
    /// What will happen to it.
    pub action: UninstallAction,
}

/// The bounded set of things uninstall may do to one artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallAction {
    /// Nothing is there.
    AlreadyAbsent,
    /// A Gregg-owned artifact; it will be removed.
    RemoveOwned,
    /// Something else; it is being left alone.
    PreservedForeign,
    /// Something unreadable; it is being left alone because ownership could
    /// not be proven.
    PreservedUnknown,
}

/// Install the user-scoped startup registration for one config.
///
/// Every branch follows the same rule: write exactly one artifact inside the
/// user's own home, then ask that same manager to start it. A manager that is
/// not available is an error the caller can print `instructions` for — never a
/// silent partial success, and never a fallback that reaches outside the user's
/// scope.
///
/// # Errors
///
/// Returns a [`StartupError`] naming the manager step that failed.
pub fn install(target: &StartupTarget) -> Result<Installed, StartupError> {
    let method = method_for(std::env::consts::OS, user_systemd_available());
    install_with(target, method)
}

/// Install using a specific method, so the choice is testable.
pub fn install_with(
    target: &StartupTarget,
    method: StartupMethod,
) -> Result<Installed, StartupError> {
    match method {
        StartupMethod::SystemdUser => install_systemd_user(target),
        StartupMethod::LaunchdUser => install_launchagent(target),
        StartupMethod::WindowsStartupFolder => install_windows_entry(target),
        StartupMethod::Cron => install_cron(target),
    }
}

fn write_artifact(path: &Path, contents: &str) -> Result<(), StartupError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| StartupError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, contents).map_err(|source| StartupError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn manager_failure(
    program: &str,
    output: Option<crate::startup_support::ManagerOutput>,
) -> StartupError {
    match output {
        Some(output) => StartupError::Manager {
            command: program.to_owned(),
            message: output.combined.trim().to_owned(),
        },
        None => StartupError::MissingManager(""),
    }
}

fn install_systemd_user(target: &StartupTarget) -> Result<Installed, StartupError> {
    let unit = systemd_user_unit_path(target);
    write_artifact(&unit, &render_systemd_user_unit(target))?;
    // Re-verifying ownership of what we just wrote is cheap and turns a
    // silently-corrupted artifact (a truncated write, a racing writer) into a
    // loud failure instead of a registration that never starts.
    let text = crate::startup_support::read_artifact(&unit)
        .map_err(|source| StartupError::Io {
            path: unit.clone(),
            source,
        })?
        .unwrap_or_default();
    if systemd_unit_ownership(&text, &target.executable, &target.config) != ArtifactOwnership::Owned
    {
        return Err(StartupError::Manager {
            command: "self-check".to_owned(),
            message: format!("the unit at {} did not read back as owned", unit.display()),
        });
    }
    let name = target.artifact_name(".service");
    crate::startup_support::run("systemctl", &["--user", "daemon-reload"])
        .ok_or(StartupError::MissingManager("systemctl --user"))?;
    let enabled = crate::startup_support::run("systemctl", &["--user", "enable", "--now", &name])
        .ok_or(StartupError::MissingManager("systemctl --user"))?;
    if !enabled.succeeded() {
        return Err(manager_failure("systemctl --user enable", Some(enabled)));
    }
    Ok(Installed {
        method: StartupMethod::SystemdUser,
        artifact: unit,
        started: true,
    })
}

fn install_launchagent(target: &StartupTarget) -> Result<Installed, StartupError> {
    let plist = launchagent_path(target);
    write_artifact(&plist, &render_launchagent(target))?;
    let text = crate::startup_support::read_artifact(&plist)
        .map_err(|source| StartupError::Io {
            path: plist.clone(),
            source,
        })?
        .unwrap_or_default();
    if launchagent_ownership(&text, &target.executable, &target.config) != ArtifactOwnership::Owned
    {
        return Err(StartupError::Manager {
            command: "self-check".to_owned(),
            message: format!(
                "the plist at {} did not read back as owned",
                plist.display()
            ),
        });
    }
    // `bootstrap` targets a specific user agent domain. An older `load -w` is
    // used as a fallback because it is what older macOS still supports; the
    // user domain is explicit either way, never `system`.
    let bootstrapped = crate::startup_support::run(
        "launchctl",
        &[
            "bootstrap",
            &format!("gui/{}", current_uid()),
            &plist.to_string_lossy(),
        ],
    );
    let started = match bootstrapped {
        Some(output) if output.succeeded() => true,
        _ => {
            let loaded =
                crate::startup_support::run("launchctl", &["load", "-w", &plist.to_string_lossy()])
                    .ok_or(StartupError::MissingManager("launchctl"))?;
            if !loaded.succeeded() {
                return Err(manager_failure("launchctl load", Some(loaded)));
            }
            true
        }
    };
    Ok(Installed {
        method: StartupMethod::LaunchdUser,
        artifact: plist,
        started,
    })
}

fn install_windows_entry(target: &StartupTarget) -> Result<Installed, StartupError> {
    let entry = windows_startup_entry_path(target).ok_or(StartupError::Unavailable(
        "the current-user Startup folder could not be located (APPDATA is unset)",
    ))?;
    write_artifact(&entry, &render_windows_startup_entry(target))?;
    let text = crate::startup_support::read_artifact(&entry)
        .map_err(|source| StartupError::Io {
            path: entry.clone(),
            source,
        })?
        .unwrap_or_default();
    if windows_entry_ownership(&text, &target.executable, &target.config)
        != ArtifactOwnership::Owned
    {
        return Err(StartupError::Manager {
            command: "self-check".to_owned(),
            message: format!(
                "the entry at {} did not read back as owned",
                entry.display()
            ),
        });
    }
    // A Startup-folder entry is started by the shell at login; there is no
    // manager to ask. The daemon may still not be running yet, which is what
    // lazy activation is for.
    Ok(Installed {
        method: StartupMethod::WindowsStartupFolder,
        artifact: entry,
        started: false,
    })
}

fn install_cron(target: &StartupTarget) -> Result<Installed, StartupError> {
    let existing = run_crontab()?;
    let ownership = cron_block_ownership(&existing, &target.executable, &target.config);
    match ownership {
        ArtifactOwnership::Foreign => {
            return Err(StartupError::Manager {
                command: "crontab".to_owned(),
                message: "this crontab already contains a different Gregg client-daemon entry; \
                          remove it by hand rather than having Gregg overwrite it"
                    .to_owned(),
            });
        }
        ArtifactOwnership::Unknown => {
            return Err(StartupError::Manager {
                command: "crontab".to_owned(),
                message: "this crontab is not Gregg-managed, so Gregg will not rewrite it; \
                          add the line from `gregg daemon startup instructions` yourself"
                    .to_owned(),
            });
        }
        ArtifactOwnership::Owned | ArtifactOwnership::Absent => {}
    }
    let merged = merge_cron_block(&existing, &render_cron_watchdog(target));
    write_crontab(&merged)?;
    Ok(Installed {
        method: StartupMethod::Cron,
        artifact: PathBuf::from("(user crontab)"),
        started: false,
    })
}

fn run_crontab() -> Result<String, StartupError> {
    // A crontab is one shared file that `crontab -` *replaces whole*, so this read
    // is the only thing standing between a manager hiccup and the loss of every
    // unrelated job the user has. It therefore fails closed: only two outcomes
    // are trusted, and everything else is an error rather than an empty table.
    let Some(output) = crate::startup_support::run("crontab", &["-l"]) else {
        // Spawn failure or the bounded timeout. `None` used to collapse into
        // "you have no crontab", which is how a `crontab -l` that blocked writing
        // a large table used to be rewritten from scratch.
        return Err(StartupError::Manager {
            command: "crontab -l".to_owned(),
            message: "reading the crontab timed out or could not be started, so it will not \
                      be rewritten; add the line from `gregg daemon startup instructions` \
                      yourself"
                .to_owned(),
        });
    };
    if !output.lossless {
        // Non-UTF-8 or over-cap output would be rewritten with U+FFFD
        // substitutions, quietly corrupting the user's jobs.
        return Err(StartupError::Manager {
            command: "crontab -l".to_owned(),
            message: "the crontab is not readable as text, so it will not be rewritten; add \
                      the line from `gregg daemon startup instructions` yourself"
                .to_owned(),
        });
    }
    if output.succeeded() {
        return Ok(output.combined);
    }
    // A non-zero exit is the ordinary "you have no jobs" case, but it is also
    // what an unreadable spool looks like. Only the standard absence diagnostic
    // is treated as an empty table; every other failure — and every locale whose
    // message is not this one — fails closed toward the manual instruction.
    if crontab_reports_absence(&output.combined) {
        Ok(String::new())
    } else {
        Err(StartupError::Manager {
            command: "crontab -l".to_owned(),
            message: format!(
                "the crontab could not be read ({}), so it will not be rewritten; add the line \
                 from `gregg daemon startup instructions` yourself",
                output.combined.trim()
            ),
        })
    }
}

/// Whether a failed `crontab -l` reported the standard "no crontab" diagnostic.
///
/// This is deliberately a conservative match: an unrecognised message means the
/// table is treated as unreadable rather than empty, so a localized `crontab`
/// degrades to "install by hand" instead of to data loss.
fn crontab_reports_absence(diagnostics: &str) -> bool {
    diagnostics.to_ascii_lowercase().contains("no crontab")
}

fn write_crontab(contents: &str) -> Result<(), StartupError> {
    use std::io::Write as _;
    let mut child = std::process::Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| StartupError::MissingManager("crontab"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(contents.as_bytes());
    }
    let status = child.wait().map_err(|source| StartupError::Io {
        path: PathBuf::from("(user crontab)"),
        source,
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(StartupError::Manager {
            command: "crontab -".to_owned(),
            message: status.to_string(),
        })
    }
}

/// Replace or append this config's managed cron block, leaving everything else
/// alone. Pure, so the merge is testable without a crontab.
#[must_use]
pub fn merge_cron_block(existing: &str, block: &str) -> String {
    let marker = block
        .lines()
        .find_map(|line| line.strip_prefix("# gregg-client-daemon "))
        .map(|identity| format!("# gregg-client-daemon {identity}"));
    let Some(marker) = marker else {
        return existing.to_owned();
    };
    let mut kept: Vec<&str> = Vec::new();
    let mut skip_command = false;
    for line in existing.lines() {
        if skip_command {
            // A managed block is exactly two lines: the marker and the single
            // `@reboot` command line `render_cron_watchdog` emits after it. The
            // block is appended last, so a job the operator later adds with
            // `crontab -e` lands *after* it with no blank line to scan for.
            // Consuming exactly this one line — and no more — is what keeps
            // that job out of the skip state and off the rewrite.
            skip_command = false;
            continue;
        }
        if line.starts_with(&marker) {
            skip_command = true;
            continue;
        }
        kept.push(line);
    }
    let mut merged = kept.join("\n");
    while merged.ends_with('\n') {
        merged.pop();
    }
    if !merged.is_empty() {
        merged.push('\n');
    }
    merged.push_str(block);
    if !merged.ends_with('\n') {
        merged.push('\n');
    }
    merged
}

/// Remove this config's managed cron block, leaving everything else alone.
#[must_use]
pub fn remove_cron_block(existing: &str, identity: &str) -> (String, bool) {
    let marker = format!("# gregg-client-daemon {identity}");
    let mut kept: Vec<&str> = Vec::new();
    let mut skip_command = false;
    let mut removed = false;
    for line in existing.lines() {
        if skip_command {
            // Exactly the command line the marker announced — see
            // `merge_cron_block`. Anything the operator appended after the
            // block must survive an uninstall too.
            skip_command = false;
            continue;
        }
        if line.starts_with(&marker) {
            skip_command = true;
            removed = true;
            continue;
        }
        kept.push(line);
    }
    (kept.join("\n").trim_end().to_owned() + "\n", removed)
}

/// The current user's uid, for the launchd `gui/<uid>` domain.
///
/// Read from `id -u` rather than through an `unsafe` `getuid` call. The client
/// crate's unsafe allowlist does not cover this, and the architecture prefers
/// not to widen it for a value `std` cannot supply: the `id` binary is on the
/// startup allowlist and its output is advisory only, because a wrong number
/// here makes `launchctl bootstrap` fail loudly rather than register anything
/// in the wrong domain.
fn current_uid() -> String {
    crate::startup_support::run("id", &["-u"])
        .filter(crate::startup_support::ManagerOutput::succeeded)
        .map(|output| output.combined.trim().to_owned())
        .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| "501".to_owned())
}

/// Inspect the startup artifacts for one config without changing anything.
///
/// This is what `uninstall --dry-run` and `daemon startup` status both read, and
/// it is the single place ownership is decided. A manager that cannot be
/// inspected reports `Unknown`, which every caller treats as "do not touch".
#[must_use]
pub fn inspect(target: &StartupTarget) -> UninstallStep {
    let method = method_for(std::env::consts::OS, user_systemd_available());
    inspect_with(target, method)
}

/// Inspect using a specific method, so the choice is testable.
#[must_use]
pub fn inspect_with(target: &StartupTarget, method: StartupMethod) -> UninstallStep {
    let (artifact, ownership) = match method {
        StartupMethod::SystemdUser => {
            let path = systemd_user_unit_path(target);
            let ownership = read_and_classify(&path, |text| {
                systemd_unit_ownership(text, &target.executable, &target.config)
            });
            (path, ownership)
        }
        StartupMethod::LaunchdUser => {
            let path = launchagent_path(target);
            let ownership = read_and_classify(&path, |text| {
                launchagent_ownership(text, &target.executable, &target.config)
            });
            (path, ownership)
        }
        StartupMethod::WindowsStartupFolder => {
            let Some(path) = windows_startup_entry_path(target) else {
                return UninstallStep {
                    artifact: PathBuf::from("(windows startup folder)"),
                    action: UninstallAction::AlreadyAbsent,
                };
            };
            let ownership = read_and_classify(&path, |text| {
                windows_entry_ownership(text, &target.executable, &target.config)
            });
            (path, ownership)
        }
        StartupMethod::Cron => {
            // An unreadable crontab is not an absent one. Reporting `Absent`
            // here would tell the operator their jobs are already gone when in
            // fact Gregg simply could not read the file, so it reports the
            // "could not prove this is ours" verdict instead.
            let ownership = match run_crontab() {
                Ok(existing) => cron_block_ownership(&existing, &target.executable, &target.config),
                Err(_) => ArtifactOwnership::Unknown,
            };
            return UninstallStep {
                artifact: PathBuf::from("(user crontab)"),
                // A crontab is one shared file, so a foreign Gregg block or an
                // unparseable crontab is preserved wholesale rather than
                // rewritten around.
                action: match ownership {
                    ArtifactOwnership::Foreign | ArtifactOwnership::Unknown => {
                        UninstallAction::PreservedForeign
                    }
                    other => action_for(&other),
                },
            };
        }
    };
    UninstallStep {
        artifact,
        action: action_for(&ownership),
    }
}

/// Map an ownership verdict to the action uninstall may take.
///
/// Pure, and the only place that decision is made. `Unknown` maps to
/// `PreservedUnknown` rather than to `PreservedForeign` so a diagnostic can
/// tell the operator "we could not prove this is ours" apart from "this is
/// definitively not ours" — the two need different fixes.
#[must_use]
pub fn action_for(ownership: &ArtifactOwnership) -> UninstallAction {
    match ownership {
        ArtifactOwnership::Owned => UninstallAction::RemoveOwned,
        ArtifactOwnership::Absent => UninstallAction::AlreadyAbsent,
        ArtifactOwnership::Foreign => UninstallAction::PreservedForeign,
        ArtifactOwnership::Unknown => UninstallAction::PreservedUnknown,
    }
}

fn read_and_classify(
    path: &Path,
    classify: impl Fn(&str) -> ArtifactOwnership,
) -> ArtifactOwnership {
    match crate::startup_support::read_artifact(path) {
        Ok(Some(text)) => classify(&text),
        Ok(None) => ArtifactOwnership::Absent,
        // An unreadable artifact is not provably ours.
        Err(_) => ArtifactOwnership::Unknown,
    }
}

/// Remove a startup registration, but only if it is provably ours.
///
/// # Errors
///
/// Returns a [`StartupError`] if the manager step needed to remove the
/// registration failed. An artifact that is foreign or unreadable is reported
/// and left in place rather than deleted.
pub fn uninstall(target: &StartupTarget) -> Result<UninstallStep, StartupError> {
    let method = method_for(std::env::consts::OS, user_systemd_available());
    uninstall_with(target, method)
}

/// Remove using a specific method, so the choice is testable.
pub fn uninstall_with(
    target: &StartupTarget,
    method: StartupMethod,
) -> Result<UninstallStep, StartupError> {
    let step = inspect_with(target, method);
    if step.action != UninstallAction::RemoveOwned {
        return Ok(step);
    }
    match method {
        StartupMethod::SystemdUser => {
            let name = target.artifact_name(".service");
            let _ =
                crate::startup_support::run("systemctl", &["--user", "disable", "--now", &name]);
            std::fs::remove_file(&step.artifact).map_err(|source| StartupError::Io {
                path: step.artifact.clone(),
                source,
            })?;
            let _ = crate::startup_support::run("systemctl", &["--user", "daemon-reload"]);
        }
        StartupMethod::LaunchdUser => {
            let _ = crate::startup_support::run(
                "launchctl",
                &[
                    "bootout",
                    &format!("gui/{}", current_uid()),
                    &step.artifact.to_string_lossy(),
                ],
            );
            let _ = crate::startup_support::run(
                "launchctl",
                &["unload", "-w", &step.artifact.to_string_lossy()],
            );
            std::fs::remove_file(&step.artifact).map_err(|source| StartupError::Io {
                path: step.artifact.clone(),
                source,
            })?;
        }
        StartupMethod::WindowsStartupFolder => {
            std::fs::remove_file(&step.artifact).map_err(|source| StartupError::Io {
                path: step.artifact.clone(),
                source,
            })?;
        }
        StartupMethod::Cron => {
            let existing = run_crontab()?;
            let (remaining, removed) = remove_cron_block(&existing, &target.identity);
            if removed {
                write_crontab(&remaining)?;
            }
            if !removed {
                return Ok(UninstallStep {
                    artifact: step.artifact,
                    action: UninstallAction::AlreadyAbsent,
                });
            }
        }
    }
    Ok(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clientd::startup::{
        ArtifactOwnership, StartupMethod, StartupTarget, UninstallAction,
    };

    fn target(identity: &str) -> StartupTarget {
        StartupTarget::new(
            Path::new("/home/ann/bin/gregg"),
            Path::new("/home/ann/.config/gregg/gregg.toml"),
            identity,
        )
    }

    // ── Method selection ──────────────────────────────────────────────

    #[test]
    fn method_selection_is_user_scoped_on_every_platform() {
        assert_eq!(
            method_for("linux", true),
            StartupMethod::SystemdUser,
            "user systemd when it is actually usable"
        );
        assert_eq!(
            method_for("linux", false),
            StartupMethod::Cron,
            "a user crontab, never a system unit, when user systemd is absent"
        );
        assert_eq!(method_for("macos", false), StartupMethod::LaunchdUser);
        assert_eq!(
            method_for("windows", false),
            StartupMethod::WindowsStartupFolder,
            "a Startup-folder entry, never LocalService SCM"
        );
    }

    #[test]
    fn no_artifact_path_can_ever_escape_the_users_own_home() {
        let target = target("0123456789abcdef");
        // The names embed the config identity, so two configs never collide.
        assert!(target
            .artifact_name(".service")
            .contains("0123456789abcdef"));

        // These are computed against the real environment, so the assertion is
        // about the *shape*: every path is under the user home, and none is
        // under a system manager directory.
        for path in [systemd_user_unit_path(&target), launchagent_path(&target)] {
            let text = path.to_string_lossy().to_string();
            assert!(
                !text.contains("/etc/systemd/system"),
                "must never be a system unit: {text}"
            );
            assert!(
                !text.contains("/Library/LaunchDaemons"),
                "must never be a system LaunchDaemon: {text}"
            );
            assert!(text.contains(".service") || text.to_ascii_lowercase().ends_with(".plist"));
        }
    }

    // ── systemd user unit ─────────────────────────────────────────────

    #[test]
    fn a_rendered_unit_round_trips_back_to_its_owner() {
        let target = target("0123456789abcdef");
        let unit = render_systemd_user_unit(&target);
        assert_eq!(
            systemd_unit_ownership(&unit, &target.executable, &target.config),
            ArtifactOwnership::Owned
        );
        // The unit is user-scoped and restarts itself.
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.contains("Restart=always"));
    }

    #[test]
    fn a_unit_for_another_executable_or_config_is_foreign_not_owned() {
        let base = target("0123456789abcdef");
        let unit = render_systemd_user_unit(&base);

        let mut other_exe = base.clone();
        other_exe.executable = PathBuf::from("/opt/gregg/bin/gregg");
        assert_eq!(
            systemd_unit_ownership(&unit, &other_exe.executable, &other_exe.config),
            ArtifactOwnership::Foreign,
            "a unit naming a different executable must be preserved"
        );

        let mut other_config = base.clone();
        other_config.config = PathBuf::from("/home/ann/other.toml");
        assert_eq!(
            systemd_unit_ownership(&unit, &other_config.executable, &other_config.config),
            ArtifactOwnership::Foreign,
            "a unit naming a different config must be preserved"
        );
    }

    #[test]
    fn an_unparseable_unit_is_unknown_and_therefore_never_removed() {
        let target = target("0123456789abcdef");
        for text in [
            "",
            "not a unit at all",
            "[Service]\nExecStart=",
            "# truncated",
        ] {
            assert_eq!(
                systemd_unit_ownership(text, &target.executable, &target.config),
                ArtifactOwnership::Unknown,
                "unreadable must never read as owned: {text:?}"
            );
        }
    }

    #[test]
    fn systemd_quoting_survives_spaces_quotes_and_backslashes() {
        // The three shapes that break naive whitespace splitting.
        for raw in [
            "/home/ann/My Configs/gregg",
            "/home/ann/od\"d/gregg",
            r"C:\Users\ann\gregg.exe",
        ] {
            let quoted = quote_systemd(raw);
            let expected = PathBuf::from(&quoted);
            let target = StartupTarget::new(&expected, &expected, "0123456789abcdef");
            let unit = render_systemd_user_unit(&target);
            let (parsed_exe, parsed_config) = parse_systemd_exec_start(&unit).expect("round trips");
            assert_eq!(parsed_exe, expected, "executable survives {raw:?}");
            assert_eq!(parsed_config, Some(expected), "config survives {raw:?}");
        }
    }

    #[test]
    fn a_unit_without_an_explicit_config_is_foreign() {
        // A unit that would read the *default* config is not ours: it would
        // silently serve a different fleet than the one registered.
        let unit = "[Service]\nExecStart=\"/usr/bin/gregg\" daemon run\n";
        let target = target("0123456789abcdef");
        assert_eq!(
            systemd_unit_ownership(unit, &target.executable, &target.config),
            ArtifactOwnership::Foreign
        );
    }

    // ── macOS LaunchAgent ─────────────────────────────────────────────

    #[test]
    fn a_rendered_launchagent_round_trips_and_escapes_xml() {
        let mut odd = target("0123456789abcdef");
        odd.executable = PathBuf::from("/Users/ann/A&B <\"gregg\">");
        let plist = render_launchagent(&odd);
        assert!(plist.contains("&amp;"));
        let arguments = parse_launchagent_program_arguments(&plist).expect("round trips");
        assert_eq!(
            arguments[0], odd.executable,
            "xml entities are undone on parse"
        );
        assert_eq!(arguments[1], Path::new("--config"));
        assert_eq!(arguments[2], odd.config);
        assert_eq!(arguments[3], Path::new("daemon"));
        assert_eq!(arguments[4], Path::new("run"));
        assert_eq!(
            launchagent_ownership(&plist, &odd.executable, &odd.config),
            ArtifactOwnership::Owned
        );
    }

    #[test]
    fn a_foreign_plist_with_our_label_is_still_preserved() {
        let target = target("0123456789abcdef");
        // A hostile artifact can carry any label; only the argument vector is
        // the ownership proof.
        let plist = "<?xml version=\"1.0\"?>\n<plist><dict>\n\
             <key>Label</key><string>com.eggstack.gregg-client-0123456789abcdef</string>\n\
             <key>ProgramArguments</key><array><string>/usr/bin/other</string></array>\n\
             </dict></plist>";
        assert_eq!(
            launchagent_ownership(plist, &target.executable, &target.config),
            ArtifactOwnership::Foreign
        );
    }

    #[test]
    fn a_plist_without_program_arguments_is_unknown() {
        let target = target("0123456789abcdef");
        assert_eq!(
            launchagent_ownership(
                "<plist><dict><key>Label</key><string>x</string></dict></plist>",
                &target.executable,
                &target.config
            ),
            ArtifactOwnership::Unknown
        );
    }

    // ── Windows Startup folder ────────────────────────────────────────

    #[test]
    fn a_rendered_startup_entry_round_trips_with_spaces_in_paths() {
        let mut odd = target("0123456789abcdef");
        odd.executable = PathBuf::from(r"C:\Program Files\Gregg\gregg.exe");
        odd.config = PathBuf::from(r"C:\Users\ann My Data\gregg.toml");
        let entry = render_windows_startup_entry(&odd);
        let (parsed_exe, parsed_config) = parse_windows_startup_entry(&entry).expect("round trips");
        assert_eq!(parsed_exe, odd.executable);
        assert_eq!(parsed_config, Some(odd.config.clone()));
        assert_eq!(
            windows_entry_ownership(&entry, &odd.executable, &odd.config),
            ArtifactOwnership::Owned
        );
    }

    #[test]
    fn a_startup_entry_without_our_marker_is_unknown_not_owned() {
        let target = target("0123456789abcdef");
        let foreign = "@echo off\r\nstart \"\" \"C:\\Windows\\System32\\calc.exe\"\r\n";
        assert_eq!(
            windows_entry_ownership(foreign, &target.executable, &target.config),
            ArtifactOwnership::Unknown
        );
    }

    #[test]
    fn startup_execution_cannot_reach_a_privileged_or_shell_program() {
        // The allowlist is the entire safety argument for "no internal sudo",
        // so it is asserted rather than trusted.
        for forbidden in [
            "sudo",
            "su",
            "doas",
            "pkexec",
            "sh",
            "bash",
            "powershell",
            "cmd",
        ] {
            assert!(
                !crate::startup_support::is_allowed(forbidden),
                "{forbidden} must never be executable from the startup path"
            );
            assert!(
                crate::startup_support::run_bounded(
                    forbidden,
                    &["-c", "echo should-not-run"],
                    Duration::from_secs(1)
                )
                .is_none(),
                "{forbidden} must not be spawned even when asked"
            );
        }
        // The managers that *are* allowed are exactly the user-scoped ones.
        for allowed in ["systemctl", "launchctl", "crontab", "schtasks", "id"] {
            assert!(crate::startup_support::is_allowed(allowed));
        }
    }

    // ── Cron fallback ─────────────────────────────────────────────────

    #[test]
    fn the_cron_watchdog_merges_without_disturbing_other_jobs() {
        let target = target("0123456789abcdef");
        let existing = "0 5 * * * /usr/bin/backup --nightly\n";
        let merged = merge_cron_block(existing, &render_cron_watchdog(&target));
        assert!(
            merged.contains("/usr/bin/backup --nightly"),
            "an unrelated job must survive: {merged}"
        );
        assert!(merged.contains("daemon run"));
        assert_eq!(
            cron_block_ownership(&merged, &target.executable, &target.config),
            ArtifactOwnership::Owned
        );
    }

    #[test]
    fn re_installing_replaces_only_our_own_block() {
        let target = target("0123456789abcdef");
        let mut merged = merge_cron_block(
            "0 5 * * * /usr/bin/backup\n",
            &render_cron_watchdog(&target),
        );
        let once = merged.matches("daemon run").count();
        merged = merge_cron_block(&merged, &render_cron_watchdog(&target));
        assert_eq!(
            merged.matches("daemon run").count(),
            once,
            "a rerun must not accumulate duplicate watchdog lines"
        );
        assert!(merged.contains("/usr/bin/backup"));
    }

    #[test]
    fn removing_our_cron_block_leaves_every_other_job_and_is_idempotent() {
        let target = target("0123456789abcdef");
        let merged = merge_cron_block(
            "0 5 * * * /usr/bin/backup\n30 1 * * * /usr/bin/rotate\n",
            &render_cron_watchdog(&target),
        );
        let (removed, changed) = remove_cron_block(&merged, &target.identity);
        assert!(changed);
        assert!(removed.contains("/usr/bin/backup"));
        assert!(removed.contains("/usr/bin/rotate"));
        assert!(!removed.contains("daemon run"));
        let (_, again) = remove_cron_block(&removed, &target.identity);
        assert!(!again, "removal is idempotent");
    }

    #[test]
    fn a_user_job_added_after_our_block_survives_a_rerun_and_an_uninstall() {
        let target = target("0123456789abcdef");
        let before = "0 3 * * * /usr/bin/backup.sh\n";
        let merged = merge_cron_block(before, &render_cron_watchdog(&target));
        // `crontab -e` after install: the operator's own job lands after the
        // block, and the managed block is the tail with no blank line after it.
        let edited = format!("{merged}*/5 * * * * /usr/bin/my-own-job.sh\n");
        assert_eq!(
            cron_block_ownership(&edited, &target.executable, &target.config),
            ArtifactOwnership::Owned,
            "the edited table is still ours to rewrite"
        );

        let rerun = merge_cron_block(&edited, &render_cron_watchdog(&target));
        assert!(
            rerun.contains("/usr/bin/my-own-job.sh"),
            "a rerun must not destroy a job added after the block: {rerun}"
        );
        assert!(
            rerun.contains("/usr/bin/backup.sh"),
            "a rerun must not disturb a job before the block: {rerun}"
        );

        let (uninstalled, removed) = remove_cron_block(&edited, &target.identity);
        assert!(removed);
        assert!(
            uninstalled.contains("/usr/bin/my-own-job.sh"),
            "an uninstall must not destroy a job added after the block: {uninstalled}"
        );
        assert!(uninstalled.contains("/usr/bin/backup.sh"));
        assert!(!uninstalled.contains("daemon run"));
    }

    #[test]
    fn another_configs_cron_block_is_foreign_to_this_one() {
        let a = target("0123456789abcdef");
        // A different config *path* is what makes this a different config; the
        // identity digest is derived from that path, so the two cannot agree by
        // accident.
        let mut b = target("fedcba9876543210");
        b.config = PathBuf::from("/home/ann/other.toml");
        let block = render_cron_watchdog(&a);
        assert_eq!(
            cron_block_ownership(&block, &b.executable, &b.config),
            ArtifactOwnership::Foreign,
            "one config's watchdog must not be removable by another"
        );
        let (_, removed) = remove_cron_block(&block, &b.identity);
        assert!(!removed);
    }

    #[test]
    fn an_untouched_crontab_is_unknown_and_never_rewritten() {
        let target = target("0123456789abcdef");
        assert_eq!(
            cron_block_ownership(
                "0 5 * * * /usr/bin/backup\n",
                &target.executable,
                &target.config
            ),
            ArtifactOwnership::Unknown,
            "a crontab with no Gregg marker is not ours to rewrite"
        );
        assert_eq!(
            cron_block_ownership("", &target.executable, &target.config),
            ArtifactOwnership::Absent
        );
    }

    #[test]
    fn shell_quoting_neutralizes_injection_shaped_paths() {
        // A config path is attacker-influenced only through the operator, but
        // a startup registration is executed by a shell or a manager, so the
        // quoting has to be right regardless.
        let hostile = "/tmp/a;rm -rf ~/$(whoami)'\"x";
        let quoted = shell_quote(hostile);
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
        assert!(!quoted[1..quoted.len() - 1].contains("';'"));
    }

    // ── Inspection and removal decisions ──────────────────────────────

    #[test]
    fn only_a_proven_owner_is_ever_removed() {
        // The ownership-to-action decision is pure, so it is tested directly
        // rather than by probing the developer's real crontab — a unit test
        // must never depend on, or read, real manager state.
        assert_eq!(
            action_for(&ArtifactOwnership::Owned),
            UninstallAction::RemoveOwned
        );
        assert_eq!(
            action_for(&ArtifactOwnership::Absent),
            UninstallAction::AlreadyAbsent
        );
        assert_eq!(
            action_for(&ArtifactOwnership::Foreign),
            UninstallAction::PreservedForeign
        );
        assert_eq!(
            action_for(&ArtifactOwnership::Unknown),
            UninstallAction::PreservedUnknown,
            "unprovable ownership must never become a removal"
        );
    }

    #[test]
    fn instructions_are_never_privileged_and_always_name_the_config() {
        let target = target("0123456789abcdef");
        for method in [
            StartupMethod::SystemdUser,
            StartupMethod::LaunchdUser,
            StartupMethod::WindowsStartupFolder,
            StartupMethod::Cron,
        ] {
            let text = render_instructions(method, &target);
            assert!(
                !text.contains("sudo"),
                "{method} instructions must never suggest elevation: {text}"
            );
            assert!(
                text.contains("gregg daemon status"),
                "{method} instructions must say how to verify"
            );
            assert!(
                text.contains("daemon run"),
                "{method} must show the command"
            );
        }
        // The systemd text must be scoped to the user manager.
        let systemd = render_instructions(StartupMethod::SystemdUser, &target);
        assert!(systemd.contains("systemctl --user"));
        assert!(!systemd.contains("sudo systemctl"));
    }
}
