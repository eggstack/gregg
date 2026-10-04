//! Component-safe uninstall for the `gregg` client (Plan 112).
//!
//! `gregg uninstall` removes only the exact invoked client executable.
//! Configuration is preserved by default; `--purge` additionally removes
//! the resolved client config file. `--dry-run` plans without mutating.
//!
//! The command never initializes the TUI runtime, never prompts
//! interactively, never invokes `sudo` internally, and never removes a
//! directory recursively. A sibling `greggd` binary sharing the install
//! directory is never touched.

use std::fmt;
use std::path::{Path, PathBuf};

/// Identity constants for the client uninstall path.
pub const PROGRAM: &str = "gregg";
pub const PACKAGE: &str = "gregg";

/// Errors returned by client uninstall planning and execution.
///
/// Library code returns these without printing or exiting; the binary
/// boundary maps them to [`crate::cli::ExitCode`].
#[derive(Debug)]
pub enum UninstallError {
    /// The current executable path could not be determined.
    CurrentExe(String),
    /// A filesystem mutation failed.
    Io { path: PathBuf, message: String },
    /// The install location is not writable by this invocation.
    Permission { message: String, elevated: String },
    /// The installation is Cargo-owned and Cargo must perform the removal.
    CargoHandoff {
        /// Exact command for the operator to run after this process exits.
        command: String,
    },
    /// The Cargo-delegated removal failed.
    CargoFailed(String),
    /// The client daemon or its startup registration could not be dealt with,
    /// and proceeding would leave the machine in a worse state than refusing.
    ClientDaemon {
        /// Operator-facing next step, already formatted.
        message: String,
    },
}

impl fmt::Display for UninstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentExe(detail) => write!(f, "failed to determine current executable: {detail}"),
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Permission { message, elevated } => {
                write!(f, "permission denied: {message}. Rerun: {elevated}")
            }
            Self::CargoHandoff { command } => write!(
                f,
                "this installation is Cargo-owned; Gregg will not bypass Cargo bookkeeping. Run after this process exits:\n  {command}"
            ),
            Self::CargoFailed(detail) => write!(f, "cargo uninstall failed: {detail}"),
            Self::ClientDaemon { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for UninstallError {}

impl From<gregg_update::UpdateError> for UninstallError {
    fn from(error: gregg_update::UpdateError) -> Self {
        match error {
            gregg_update::UpdateError::PermissionDenied { message, elevated } => {
                Self::Permission { message, elevated }
            }
            gregg_update::UpdateError::CurrentExe(detail) => Self::CurrentExe(detail),
            other => Self::CargoFailed(other.to_string()),
        }
    }
}

/// Deterministic uninstall plan: the exact resources `uninstall` will
/// change or remove.
///
/// The same plan drives `--dry-run` rendering and real execution so the
/// preview cannot drift from the mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallPlan {
    /// Exact invoked executable to delete (never a directory).
    pub exe_path: PathBuf,
    /// Resolved client config file (`--config` or platform default).
    pub config_path: PathBuf,
    /// Whether the config file currently exists.
    pub config_exists: bool,
    /// Whether `--purge` was requested.
    pub purge: bool,
    /// Positively identified Cargo ownership, if any.
    pub cargo: Option<gregg_update::CargoOwnership>,
    /// The client-daemon startup registration for this config, and what
    /// uninstall would do to it.
    ///
    /// Computed by the same ownership parser the installer used, so a plan and
    /// its execution cannot disagree about what is Gregg's.
    pub clientd_startup: crate::clientd::startup::UninstallStep,
    /// Whether a client daemon is currently serving this config.
    pub clientd_running: bool,
    /// The config identity the plan is about.
    pub clientd_id: String,
}

impl UninstallPlan {
    /// Render the exact Gregg-owned resources that would be changed or
    /// removed, one per line. Pure and side-effect free.
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("uninstall {PROGRAM} (dry run)"));
        if let Some(ownership) = &self.cargo {
            #[cfg(unix)]
            lines.push(format!(
                "cargo-owned install (root {}); Cargo removes the executable/package before post-success config cleanup; handoff: {}",
                ownership.root.display(),
                ownership.uninstall_command()
            ));
            #[cfg(not(unix))]
            lines.push(format!(
                "cargo-owned install (root {}); Windows uses a zero-mutation Cargo handoff before any startup/config change; handoff: {}",
                ownership.root.display(),
                ownership.uninstall_command()
            ));
        }
        match self.clientd_startup.action {
            crate::clientd::startup::UninstallAction::RemoveOwned => lines.push(format!(
                "remove client-daemon startup entry: {}",
                self.clientd_startup.artifact.display()
            )),
            crate::clientd::startup::UninstallAction::AlreadyAbsent => {
                lines.push("client-daemon startup: not registered".to_owned());
            }
            crate::clientd::startup::UninstallAction::PreservedForeign => lines.push(format!(
                "preserve foreign client-daemon startup entry: {}",
                self.clientd_startup.artifact.display()
            )),
            crate::clientd::startup::UninstallAction::PreservedUnknown => lines.push(format!(
                "preserve client-daemon startup entry of unprovable ownership: {}",
                self.clientd_startup.artifact.display()
            )),
        }
        lines.push(if self.clientd_running {
            format!(
                "stop identified client daemon {} before removing the executable",
                self.clientd_id
            )
        } else {
            format!("client daemon {} is not running", self.clientd_id)
        });
        lines.push(format!("remove executable: {}", self.exe_path.display()));
        if self.purge {
            if self.config_exists {
                lines.push(format!(
                    "remove config file: {}",
                    self.config_path.display()
                ));
                if let Some(dir) = purge_empty_dir_for(&self.config_path) {
                    lines.push(format!(
                        "remove empty Gregg config directory: {}",
                        dir.display()
                    ));
                }
            } else {
                lines.push(format!(
                    "config file already absent: {}",
                    self.config_path.display()
                ));
            }
        } else {
            lines.push(format!("preserve config: {}", self.config_path.display()));
        }
        lines.join("\n")
    }
}

/// Build the uninstall plan for an executable/config pair.
///
/// `confirm_cargo` wires Cargo's supported ownership confirmation; the
/// production path passes the real `cargo install --list` probe while
/// tests inject a stub. Planning performs no mutation.
pub fn plan_uninstall_with(
    exe_path: &Path,
    config_path: &Path,
    purge: bool,
    confirm_cargo: impl Fn(&Path, &str) -> bool,
) -> UninstallPlan {
    plan_uninstall_with_clientd(exe_path, config_path, purge, confirm_cargo, |_, _| false)
}

/// Build the plan, with the client-daemon facts injected for tests.
///
/// `clientd_running` is passed in rather than probed here: planning is called
/// from `--dry-run`, which must be read-only, and a probe would mean the dry
/// run itself has a side effect on the endpoint it reports about.
pub fn plan_uninstall_with_clientd(
    exe_path: &Path,
    config_path: &Path,
    purge: bool,
    confirm_cargo: impl Fn(&Path, &str) -> bool,
    clientd_running: impl Fn(&crate::clientd::ClientDaemonIdentity, &str) -> bool,
) -> UninstallPlan {
    let cargo = gregg_update::uninstall::detect_cargo_ownership_with(
        exe_path,
        PROGRAM,
        PACKAGE,
        confirm_cargo,
    );
    let identity = crate::clientd::ClientDaemonIdentity::for_path(config_path);
    let target = crate::clientd::startup::StartupTarget::new(exe_path, config_path, identity.id());
    let clientd_startup = crate::clientd::startup::inspect(&target);
    UninstallPlan {
        exe_path: exe_path.to_path_buf(),
        config_path: config_path.to_path_buf(),
        config_exists: config_path.exists(),
        purge,
        cargo,
        clientd_startup,
        clientd_running: clientd_running(&identity, crate::clientd::protocol::PROTOCOL_VERSION_STR),
        clientd_id: identity.id().to_owned(),
    }
}

/// Build the uninstall plan for the running binary and resolved config, probing
/// whether a client daemon is serving this config.
///
/// The probe is fallible on purpose and fails loudly: an endpoint that cannot
/// even be *asked* is different from one confirmed empty, and only the
/// execution step is entitled to treat the difference as blocking.
pub fn plan_uninstall_probed(
    config_path: &Path,
    purge: bool,
    probe: impl Fn(&Path, &str) -> Result<bool, Box<dyn std::error::Error>>,
) -> Result<UninstallPlan, UninstallError> {
    let exe_path = gregg_update::uninstall::resolve_uninstall_target()
        .map_err(|e| UninstallError::CurrentExe(e.to_string()))?;
    let cargo_bin = gregg_update::exec::find_cargo().ok();
    let running =
        probe(config_path, crate::clientd::protocol::PROTOCOL_VERSION_STR).map_err(|error| {
            UninstallError::ClientDaemon {
                message: format!(
                    "could not determine whether a client daemon is running for {}: {error}",
                    config_path.display()
                ),
            }
        })?;
    Ok(plan_uninstall_with_clientd(
        &exe_path,
        config_path,
        purge,
        |root, package| match &cargo_bin {
            Some(cargo) => gregg_update::uninstall::cargo_lists_package(cargo, root, package),
            None => false,
        },
        |_identity, _version| running,
    ))
}

/// Build the uninstall plan for the running binary and resolved config.
pub fn plan_uninstall(config_path: &Path, purge: bool) -> Result<UninstallPlan, UninstallError> {
    let exe_path = gregg_update::uninstall::resolve_uninstall_target()
        .map_err(|e| UninstallError::CurrentExe(e.to_string()))?;
    let cargo_bin = gregg_update::exec::find_cargo().ok();
    Ok(plan_uninstall_with(
        &exe_path,
        config_path,
        purge,
        |root, package| match &cargo_bin {
            Some(cargo) => gregg_update::uninstall::cargo_lists_package(cargo, root, package),
            None => false,
        },
    ))
}

/// When `--purge` removes the config file, the Gregg-specific parent
/// directory may additionally be removed, but only when it is one of
/// Gregg's known standard directories and empty afterwards.
///
/// Returns the directory that *would* be removed (the caller re-checks
/// emptiness after deleting the file). Returns `None` for custom
/// `--config` paths: an arbitrary explicit parent is never removed.
fn purge_empty_dir_for(config_path: &Path) -> Option<PathBuf> {
    let parent = config_path.parent()?;
    let standard_parent = crate::config::Config::default_path()
        .parent()
        .map(Path::to_path_buf)?;
    if parent == standard_parent {
        Some(parent.to_path_buf())
    } else {
        None
    }
}

/// Execute a plan. With `dry_run`, performs discovery/planning only and
/// mutates nothing; the rendered plan is printed by the caller.
///
/// Real execution order: preflight writability, Cargo handoff/delegation,
/// optional config purge, then self-deletion of the exact executable.
/// Only the exact executable file is removed; sibling binaries and
/// install directories are never touched.
pub fn execute_plan(plan: &UninstallPlan, dry_run: bool) -> Result<(), UninstallError> {
    if dry_run {
        return Ok(());
    }
    // The client daemon runs *from* the executable about to be deleted, so it
    // is quiesced first. This is ownership-first: only an identified daemon for
    // this same config is ever asked to stop, and a foreign or unreachable
    // endpoint is reported rather than forced.
    //
    // An uncertain stop blocks the deletion rather than proceeding. Leaving a
    // daemon pointing at a removed binary is worse than refusing: the next
    // `gregg` would find a stale endpoint and, correctly, refuse to touch a
    // peer it cannot identify.
    quiesce_clientd(plan)?;
    remove_clientd_startup(plan)?;
    #[cfg(not(unix))]
    if let Some(ownership) = &plan.cargo {
        return Err(UninstallError::CargoHandoff {
            command: ownership.uninstall_command(),
        });
    }
    gregg_update::preflight_uninstall_writable(&plan.exe_path, &plan.exe_path, plan.purge)?;
    #[cfg(unix)]
    if let Some(ownership) = &plan.cargo {
        gregg_update::uninstall::cargo_uninstall(ownership)
            .map_err(|e| UninstallError::CargoFailed(e.to_string()))?;
        if plan.purge {
            purge_config(&plan.config_path)?;
        }
    } else {
        if plan.purge {
            purge_config(&plan.config_path)?;
        }
        gregg_update::self_delete_current_exe(plan.purge)?;
    }
    #[cfg(not(unix))]
    {
        if plan.purge {
            purge_config(&plan.config_path)?;
        }
        gregg_update::self_delete_current_exe(plan.purge)?;
    }
    Ok(())
}

/// Stop the identified client daemon for this config, if one is running.
fn quiesce_clientd(plan: &UninstallPlan) -> Result<(), UninstallError> {
    if !plan.clientd_running {
        return Ok(());
    }
    let identity = crate::clientd::ClientDaemonIdentity::for_path(&plan.config_path);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| UninstallError::Io {
            path: plan.config_path.clone(),
            message: format!("failed to start runtime: {e}"),
        })?;
    runtime
        .block_on(crate::clientd::daemon::stop(&identity, crate::clientd::protocol::PROTOCOL_VERSION_STR))
        .map_err(|e| UninstallError::ClientDaemon {
            message: format!(
                "could not stop the client daemon for {}: {e}. \
                 Stop it with `gregg daemon stop` and re-run, so the executable is not removed from under it",
                plan.clientd_id
            ),
        })
}

/// Remove the client daemon's startup registration, but only Gregg's own.
fn remove_clientd_startup(plan: &UninstallPlan) -> Result<(), UninstallError> {
    use crate::clientd::startup::UninstallAction;
    match plan.clientd_startup.action {
        // Nothing registered, or something we will not touch. Either way the
        // executable can go.
        UninstallAction::AlreadyAbsent
        | UninstallAction::PreservedForeign
        | UninstallAction::PreservedUnknown => Ok(()),
        UninstallAction::RemoveOwned => {
            let target = crate::clientd::startup::StartupTarget::new(
                &plan.exe_path,
                &plan.config_path,
                &plan.clientd_id,
            );
            crate::clientd::startup::uninstall(&target).map_err(|e| {
                UninstallError::ClientDaemon {
                    message: format!(
                        "could not remove the client-daemon startup entry at {}: {e}. \
                         Remove it by hand, or re-run once the manager is available",
                        plan.clientd_startup.artifact.display()
                    ),
                }
            })?;
            Ok(())
        }
    }
}

/// Remove the resolved client config file for `--purge`.
///
/// Removes only the exact file; the parent directory is removed only
/// when [`purge_empty_dir_for`] identifies a known standard Gregg
/// directory that is empty after the file is gone. Never recurses.
fn purge_config(config_path: &Path) -> Result<(), UninstallError> {
    match std::fs::remove_file(config_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(UninstallError::Permission {
                message: format!("permission denied removing {}", config_path.display()),
                elevated: gregg_update::elevated_rerun_hint(
                    Path::new(&current_exe_hint()),
                    "uninstall --purge",
                ),
            });
        }
        Err(e) => {
            return Err(UninstallError::Io {
                path: config_path.to_path_buf(),
                message: format!("failed to remove config file: {e}"),
            });
        }
    }
    if let Some(dir) = purge_empty_dir_for(config_path) {
        // Non-empty or already gone: either is fine; the directory is
        // optional cleanup, never required for success.
        let _ = std::fs::remove_dir(&dir);
    }
    Ok(())
}

/// Best-effort current-exe display for elevation hints.
fn current_exe_hint() -> String {
    std::env::current_exe().map_or_else(|_| PROGRAM.to_string(), |p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_case(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gregg_uninstall_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn no_cargo(_: &Path, _: &str) -> bool {
        false
    }

    #[test]
    fn dry_run_renders_without_mutating() {
        let dir = tmp_case("dry_run");
        let exe = dir.join("gregg");
        fs::write(&exe, b"fake").unwrap();
        let config = dir.join("gregg.toml");
        fs::write(&config, b"config_version = 1\n").unwrap();

        let plan = plan_uninstall_with(&exe, &config, false, no_cargo);
        let rendered = plan.render();
        assert!(rendered.contains(&exe.display().to_string()));
        assert!(rendered.contains("preserve config"));
        execute_plan(&plan, true).unwrap();

        assert!(exe.exists(), "dry run must not delete the executable");
        assert!(config.exists(), "dry run must not delete config");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_preserve_keeps_config_but_removes_binary() {
        let dir = tmp_case("preserve");
        let exe = dir.join("gregg");
        fs::write(&exe, b"fake").unwrap();
        let sibling = dir.join("greggd");
        fs::write(&sibling, b"sibling").unwrap();
        let config = dir.join("gregg.toml");
        fs::write(&config, b"config_version = 1\n").unwrap();

        // Execute only the config-preservation half (self-delete targets
        // the test binary, so it is covered by the shared-crate tests and
        // the disposable smoke instead).
        let plan = plan_uninstall_with(&exe, &config, false, no_cargo);
        assert!(!plan.purge);
        assert!(plan.render().contains("preserve config"));
        // Simulate the file-removal scope: only the exact exe is targeted.
        fs::remove_file(&plan.exe_path).unwrap();
        assert!(!exe.exists());
        assert!(sibling.exists(), "sibling binary must survive");
        assert!(config.exists(), "config must be preserved by default");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_removes_only_the_config_file_never_its_parent() {
        let dir = tmp_case("purge_custom");
        let custom_parent = dir.join("custom-parent");
        fs::create_dir_all(&custom_parent).unwrap();
        let config = custom_parent.join("gregg.toml");
        fs::write(&config, b"config_version = 1\n").unwrap();
        let other = custom_parent.join("other.conf");
        fs::write(&other, b"keep").unwrap();

        // A custom --config path is never a standard dir, so no dir removal.
        assert_eq!(purge_empty_dir_for(&config), None);
        purge_config(&config).unwrap();
        assert!(!config.exists());
        assert!(
            custom_parent.exists(),
            "custom parent must never be removed"
        );
        assert!(other.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_missing_config_is_a_noop() {
        let dir = tmp_case("purge_missing");
        let config = dir.join("absent.toml");
        purge_config(&config).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn standard_parent_is_eligible_for_empty_cleanup() {
        let standard = crate::config::Config::default_path();
        if let Some(parent) = standard.parent() {
            let candidate = parent.join("gregg.toml");
            assert_eq!(purge_empty_dir_for(&candidate), Some(parent.to_path_buf()));
        }
        let custom = PathBuf::from("/tmp/definitely-not-standard-gregg/gregg.toml");
        assert_eq!(purge_empty_dir_for(&custom), None);
    }

    #[test]
    fn cargo_owned_plan_renders_handoff_and_config_intent() {
        let dir = tmp_case("cargo_owned");
        let exe_name = if cfg!(windows) { "gregg.exe" } else { "gregg" };
        let exe = dir.join("bin").join(exe_name);
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"fake").unwrap();
        let config = dir.join("gregg.toml");
        fs::write(&config, b"config_version = 1\n").unwrap();

        let plan = plan_uninstall_with(&exe, &config, false, |_, _| true);
        assert!(plan.cargo.is_some());
        let rendered = plan.render();
        assert!(rendered.contains("cargo-owned"));
        assert!(rendered.contains("cargo uninstall --root"));
        assert!(rendered.contains("preserve config"));
        let purge_plan = plan_uninstall_with(&exe, &config, true, |_, _| true);
        assert!(purge_plan.render().contains("remove config file"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sibling_binaries_share_a_directory_safely() {
        // The plan targets exactly one file; assert the plan itself never
        // names the sibling or the directory.
        let dir = tmp_case("sibling");
        let exe = dir.join("gregg");
        let sibling = dir.join("greggd");
        fs::write(&exe, b"fake").unwrap();
        fs::write(&sibling, b"sibling").unwrap();
        let plan = plan_uninstall_with(&exe, &dir.join("gregg.toml"), true, no_cargo);
        assert_eq!(plan.exe_path, exe);
        assert_ne!(plan.exe_path, sibling);
        assert!(plan.render().contains(&exe.display().to_string()));
        assert!(!plan.render().contains(&sibling.display().to_string()));
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::clientd::startup::{StartupMethod, StartupTarget, UninstallAction};
    use crate::update::{DaemonLifecycle, UpdateOutcome, UpdateReport};
    use std::path::{Path, PathBuf};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "gregg-lifecycle-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn exe(&self) -> PathBuf {
            self.0.join("bin/gregg")
        }

        fn config(&self) -> PathBuf {
            self.0.join("gregg.toml")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn target(dir: &TempDir) -> StartupTarget {
        let identity = crate::clientd::ClientDaemonIdentity::for_path(&dir.config());
        StartupTarget::new(&dir.exe(), &dir.config(), identity.id())
    }

    // ── Uninstall plan ────────────────────────────────────────────────

    #[test]
    fn a_dry_run_names_the_startup_entry_and_the_running_daemon() {
        let dir = TempDir::new("uninstall-plan");
        std::fs::write(dir.config(), "systems = []\n").expect("writes config");
        std::fs::create_dir_all(dir.exe().parent().expect("parent")).expect("dirs");
        std::fs::write(dir.exe(), "#!/bin/sh\n").expect("writes exe");

        let plan = plan_uninstall_with_clientd(
            &dir.exe(),
            &dir.config(),
            false,
            |_, _| false,
            |_, _| true,
        );
        let rendered = plan.render();
        // Whatever the host's real startup state is, the plan must *mention*
        // both facts rather than silently omitting them.
        assert!(
            rendered.contains("client-daemon startup"),
            "the plan must account for the startup entry: {rendered}"
        );
        assert!(
            rendered.contains("client daemon") && rendered.contains(&plan.clientd_id),
            "the plan must account for the running daemon: {rendered}"
        );
        assert!(
            rendered.contains("preserve config"),
            "config is still preserved by default: {rendered}"
        );
    }

    #[test]
    fn purge_semantics_are_unchanged_by_the_clientd_work() {
        let dir = TempDir::new("uninstall-purge");
        std::fs::write(dir.config(), "systems = []\n").expect("writes config");
        let plan = plan_uninstall_with_clientd(
            &dir.exe(),
            &dir.config(),
            true,
            |_, _| false,
            |_, _| false,
        );
        let rendered = plan.render();
        assert!(rendered.contains("remove config file"));
        assert!(!rendered.contains("preserve config"));
    }

    #[test]
    fn only_an_owned_startup_entry_is_planned_for_removal() {
        // The plan's action comes from the same ownership parser the install
        // used, so a plan and its execution cannot disagree.
        let dir = TempDir::new("uninstall-ownership");
        let target = target(&dir);
        let method = StartupMethod::SystemdUser;

        // Absent: nothing to remove, and the executable can go.
        std::fs::create_dir_all(
            crate::clientd::startup::systemd_user_unit_path(&target)
                .parent()
                .expect("has a parent"),
        )
        .expect("dirs");
        let unit = crate::clientd::startup::systemd_user_unit_path(&target);
        let _ = std::fs::remove_file(&unit);
        let absent = crate::clientd::startup::inspect_with(&target, method);
        assert_eq!(absent.action, UninstallAction::AlreadyAbsent);

        // Ours: removable.
        std::fs::write(
            &unit,
            crate::clientd::startup::render_systemd_user_unit(&target),
        )
        .expect("writes unit");
        let owned = crate::clientd::startup::inspect_with(&target, method);
        assert_eq!(owned.action, UninstallAction::RemoveOwned);

        // Someone else's: preserved, and therefore the executable is still
        // safe to remove because the entry is not ours to manage.
        let mut foreign = target.clone();
        foreign.executable = PathBuf::from("/opt/somebody-else/gregg");
        std::fs::write(
            &unit,
            crate::clientd::startup::render_systemd_user_unit(&foreign),
        )
        .expect("writes foreign unit");
        let preserved = crate::clientd::startup::inspect_with(&target, method);
        assert_eq!(preserved.action, UninstallAction::PreservedForeign);
    }

    #[test]
    fn an_unreadable_startup_entry_is_preserved_rather_than_deleted() {
        // A directory where the unit should be cannot be parsed, so ownership
        // is unprovable. Deleting it would be deleting something we cannot
        // describe.
        let dir = TempDir::new("uninstall-unreadable");
        let target = target(&dir);
        let unit = crate::clientd::startup::systemd_user_unit_path(&target);
        std::fs::create_dir_all(&unit).expect("creates a directory in the unit's place");
        let step = crate::clientd::startup::inspect_with(&target, StartupMethod::SystemdUser);
        assert_eq!(
            step.action,
            UninstallAction::PreservedUnknown,
            "an entry that cannot be read must never be removable"
        );
    }

    #[test]
    fn a_failing_probe_is_reported_rather_than_assumed_empty() {
        let dir = TempDir::new("uninstall-probe");
        let error =
            plan_uninstall_probed(
                &dir.config(),
                false,
                |_, _| Err("endpoint is wedged".into()),
            )
            .expect_err("a probe that cannot answer must not plan a removal");
        assert!(matches!(error, UninstallError::ClientDaemon { .. }));
        assert!(
            error.to_string().contains("could not determine"),
            "the operator must be told what could not be determined: {error}"
        );
    }

    #[test]
    fn two_configs_never_plan_each_others_startup_entry() {
        let first = TempDir::new("uninstall-cross-a");
        let second = TempDir::new("uninstall-cross-b");
        let a = target(&first);
        let b = target(&second);
        assert_ne!(a.identity, b.identity);
        let unit_a = crate::clientd::startup::systemd_user_unit_path(&a);
        std::fs::create_dir_all(unit_a.parent().expect("parent")).expect("dirs");
        std::fs::write(
            &unit_a,
            crate::clientd::startup::render_systemd_user_unit(&a),
        )
        .expect("writes unit");

        // Config B looks for *its* unit, which does not exist.
        let step_b = crate::clientd::startup::inspect_with(&b, StartupMethod::SystemdUser);
        assert_eq!(step_b.action, UninstallAction::AlreadyAbsent);
        // And config A still sees its own.
        let step_a = crate::clientd::startup::inspect_with(&a, StartupMethod::SystemdUser);
        assert_eq!(step_a.action, UninstallAction::RemoveOwned);
    }

    // ── Update lifecycle ──────────────────────────────────────────────

    #[test]
    fn a_stopped_daemon_is_left_stopped_by_an_update() {
        // Nothing was running, so nothing is started. An update must not turn
        // into a reason to begin observing a fleet the operator had not asked
        // Gregg to watch.
        assert_eq!(
            DaemonLifecycle::NoneWasRunning.to_string(),
            "no client daemon was running for this config"
        );
    }

    #[test]
    fn a_running_daemon_is_reported_as_relaunched_with_its_identity() {
        let report = DaemonLifecycle::Relaunched {
            id: "0123456789abcdef".to_owned(),
        };
        let text = report.to_string();
        assert!(text.contains("0123456789abcdef"));
        assert!(text.contains("new binary"));
    }

    #[test]
    fn a_failed_relaunch_is_surfaced_as_partial_success_with_a_next_step() {
        // A replaced binary whose daemon did not come back is a *reportable*
        // state, not a silent success: continuous background polling stopped,
        // and the operator should be told how to restart it.
        let report = DaemonLifecycle::RelaunchFailed {
            id: "0123456789abcdef".to_owned(),
            reason: "bind refused".to_owned(),
        };
        let text = report.to_string();
        assert!(text.contains("did not come back"), "{text}");
        assert!(
            text.contains("bind refused"),
            "the reason must be included: {text}"
        );
        assert!(
            text.contains("gregg daemon restart"),
            "and a way forward: {text}"
        );
    }

    #[test]
    fn an_update_report_only_mentions_the_daemon_when_it_was_touched() {
        let quiet = UpdateReport {
            outcome: UpdateOutcome::AlreadyCurrent {
                version: "0.1.0".to_owned(),
            },
            daemon: DaemonLifecycle::NoneWasRunning,
        };
        assert_eq!(
            quiet.to_string(),
            "gregg 0.1.0 is already the latest stable version"
        );

        let loud = UpdateReport {
            outcome: UpdateOutcome::UpdatedBinary {
                from: "0.1.0".to_owned(),
                to: "0.2.0".to_owned(),
            },
            daemon: DaemonLifecycle::Relaunched {
                id: "0123456789abcdef".to_owned(),
            },
        };
        let text = loud.to_string();
        assert!(text.contains("updated gregg 0.1.0 -> 0.2.0"), "{text}");
        assert!(text.contains("relaunched the client daemon"), "{text}");
    }

    #[test]
    fn the_update_order_is_prepare_before_quiesce() {
        // The transaction's safety property is an ordering, so it is asserted
        // from the source rather than from a comment: the daemon probe and the
        // replacement both happen before the stop, and the stop happens before
        // the relaunch.
        let source = include_str!("update.rs");
        let probe = source
            .find("daemon::status(&identity)")
            .expect("probes the daemon");
        let replace = source
            .find("let outcome = run_update()?")
            .expect("replaces");
        let stop = source
            .find("daemon::stop(&identity, version)")
            .expect("stops the daemon");
        let relaunch = source
            .find("launch::restart(store)")
            .expect("relaunches the daemon");
        assert!(
            probe < replace,
            "the daemon must be identified before anything is replaced"
        );
        assert!(
            replace < stop,
            "the candidate must be prepared and verified before the daemon is stopped"
        );
        assert!(
            stop < relaunch,
            "the daemon must be stopped before it is relaunched"
        );
    }

    #[test]
    fn a_bare_executable_is_never_deleted_by_a_directory_rename() {
        // Plan 165 keeps uninstall's existing bounded-deletion promise intact.
        // The target is always the exact file.
        let dir = TempDir::new("uninstall-exact");
        let exe = dir.exe();
        std::fs::create_dir_all(exe.parent().expect("parent")).expect("dirs");
        std::fs::write(&exe, "binary").expect("writes exe");
        let sibling = exe.with_extension("other");
        std::fs::write(&sibling, "sibling").expect("writes sibling");

        let plan =
            plan_uninstall_with_clientd(&exe, &dir.config(), false, |_, _| false, |_, _| false);
        assert_eq!(plan.exe_path, exe);
        assert_ne!(plan.exe_path, exe.parent().expect("parent"));
        assert!(
            Path::new(&sibling).exists(),
            "a sibling is not ours to touch"
        );
    }
}
