# Plan 165: Gregg client-daemon lifecycle, install, update, and uninstall

Status: planned.

Depends on: completed Plan 164 and the settled updater/installer ownership work
from Plans 099-116 and 130-131. Independent of Plan 091.

## Objective

Make the Plan-164 client daemon operationally invisible during normal Gregg use:
bare gregg ensures the matching daemon is running, user-scoped startup can keep
it alive when no TUI has been opened, and update/uninstall never knowingly leave
an incompatible owned daemon behind.

Reuse proven Gregg lifecycle concepts where they fit, but keep the client daemon
per-user rather than turning it into another privileged greggd.

## Bare gregg lazy activation

Before entering the TUI:

1. resolve the selected Gregg config path/identity;
2. perform a bounded local IPC handshake;
3. if a compatible matching daemon answers, attach;
4. if the endpoint is absent/refused under an absence classification, acquire a
   config-specific launch lock;
5. re-probe after the lock to avoid TOCTOU double launch;
6. detached-spawn the exact current executable as gregg daemon run with the
   exact selected config;
7. wait boundedly for the local handshake to become ready;
8. attach or return an actionable error.

Do not infer daemon identity from process name, PID scanning, or socket
existence alone.

A malformed/foreign/incompatible peer is not absent and must not authorize a
competing daemon until ownership/version handling below resolves it.

## Multi-TUI launch race

Two bare Gregg commands started simultaneously against a stopped daemon must
produce one daemon.

Use a config-specific advisory/file lock or an equivalent native single-launch
primitive. The lock is held only across probe/spawn/readiness coordination, not
for daemon lifetime.

Tests must deterministically force the race and prove exactly one spawn
authority wins.

## Compatibility/version handshake

The local IPC protocol needs an explicit major/version independent of the
remote greggd wire schema.

On attach:

- compatible protocol -> connect;
- incompatible but positively identified owned Gregg client daemon using the
  same config -> stop/restart it with the exact current executable, then retry;
- foreign/unknown local peer -> fail closed with diagnostic;
- newer daemon that cannot safely talk to the older frontend -> fail with
  upgrade/downgrade guidance rather than killing it blindly.

This self-healing attach path handles long-lived daemons across a client binary
update without requiring a global persistent registry of every active explicit
config.

## Daemon commands

Extend the Plan-164 command family with ownership-aware diagnostics/lifecycle:

~~~text
gregg daemon status
gregg daemon stop
gregg daemon restart
gregg daemon startup install
gregg daemon startup instructions
~~~

status is read-only and verifies local protocol/config identity.

stop only stops the matching identified daemon.

restart stops/relaunches only an owned matching daemon.

No internal sudo.

## User-scoped startup

The client daemon belongs to the user whose Gregg config it reads.

### Linux

Prefer user systemd when available.

The unit:

- runs the exact Gregg executable;
- passes daemon run and the selected config;
- is user-scoped;
- does not create a system service user;
- does not require root.

If user systemd is unavailable, qualify one bounded fallback suitable for a
user process, such as a managed user crontab watchdog/reboot policy. Do not copy
greggd's system-unit assumptions.

### macOS

Use a LaunchAgent owned by the user, not
/Library/LaunchDaemons/com.eggstack.greggd.plist.

Keep the label config-specific when explicit configs are supported for startup.

### Windows

Use a current-user startup mechanism appropriate to a background user process.
Do not install the client daemon as LocalService SCM merely because greggd uses
SCM.

Prefer a native mechanism already available without a large dependency. If the
choice is Task Scheduler versus startup registration, record ownership,
uninstall discoverability, and no-admin behavior before implementation.

## Default versus explicit config startup

Support the default config first-class.

For explicit --config startup registration, derive a stable bounded manager
identity from the normalized config identity so two registrations do not
collide.

Manager artifacts must record enough command identity to distinguish owned
Gregg registrations from foreign files/tasks with a similar name.

Do not create a global registry/database only to enumerate configs.

## Installer behavior

Reconcile packaging/install.sh and Windows installer semantics.

### User-local Gregg install

When installing gregg as the current unprivileged user:

- it may install/refresh the default-config user startup registration after the
  binary is fully acquired and verified;
- failure to register startup should leave the binary installed and print the
  exact gregg daemon startup install command;
- it must not mutate system manager state.

### System-wide binary install

When an administrator/root installs gregg into a shared path:

- do not guess which human user's config/client daemon should be registered;
- install the binary only;
- each user gets lazy activation on first invocation and may run their own
  gregg daemon startup install.

This distinction must be documented rather than silently creating root's client
daemon.

Cargo installs remain source/package installation and need not auto-register a
manager; lazy activation still makes bare Gregg usable.

## Update semantics

Reuse gregg-update for binary acquisition/verification exactly as today.

Lifecycle changes occur only after the candidate is fully prepared.

For the selected config:

1. identify whether a compatible/owned client daemon is running;
2. prepare and verify update candidate;
3. quiesce only the owned matching daemon if replacement requires lifecycle
   coordination;
4. replace exact executable;
5. relaunch the selected daemon if it was previously active;
6. verify local handshake with the new binary;
7. surface partial-success if replacement succeeded but relaunch failed.

Other explicit-config daemons using the same replaced executable may continue
running the old mapped image on platforms where replacement permits it. They
must be reconciled on their next attach through the version handshake rather
than requiring a persistent global daemon registry.

If platform semantics make replacing the running executable impossible, use
the same ownership-first prepare-before-stop transaction discipline established
for greggd.

## Uninstall semantics

gregg uninstall must now account for the client daemon before deleting the
executable.

Requirements:

- dry-run lists owned selected/default client-daemon manager artifact and
  running daemon action;
- stop only an identified owned matching daemon;
- remove only startup artifacts whose parsed command target matches the exact
  executable/config identity;
- preserve foreign/ambiguous manager artifacts;
- config preserved by default;
- --purge retains its existing bounded config-file meaning;
- no recursive directory deletion;
- uncertain stop blocks executable removal when the running process would make
  removal unsafe or incorrect on that platform.

For multiple explicit startup registrations, discover only manager artifacts in
Gregg's narrowly owned namespace; do not scan arbitrary user services/tasks.

## Restart after installer replacement

The bootstrap installer's same-scope replacement path must preserve an already
running user client daemon similarly to the existing user-local greggd
reactivation logic, but using local IPC identity instead of HTTP health.

Acquisition must complete before stop/restart.

First install of gregg may register user startup as described above but does
not need to launch an interactive TUI.

## No idle shutdown

Do not stop clientd merely because the last TUI disconnects. Continuous
background polling/history capture is the purpose of the architecture.

Resource use is controlled through existing poll cadence and Plan-167
measurement, not an idle timeout.

## Deterministic tests

Cover:

- bare Gregg attaches to existing compatible daemon;
- absent daemon -> one bounded spawn -> ready;
- simultaneous two-client launch race -> one daemon;
- foreign local endpoint fails closed;
- incompatible owned old daemon rotates to current binary;
- daemon status/stop exact config targeting;
- two configs cannot cross-stop;
- user startup artifact ownership parsing;
- system-wide install does not register root/current-user clientd accidentally;
- user-local installer registration failure is actionable/non-destructive;
- update prepare-before-stop;
- running/stopped update state preservation;
- update partial-success on restart failure;
- uninstall dry-run and owned teardown;
- foreign startup artifact preservation;
- no last-TUI idle shutdown.

Use isolated HOME/runtime dirs in tests; never mutate the developer's actual
systemd/launchd/Task Scheduler state in ordinary unit tests.

## Native evidence

Use existing platform CI for compile/native truth.

One focused operational smoke per manager class is appropriate where the
existing CI environment supports it, but do not add a new workflow/job merely
to prove startup registration unless the current jobs cannot establish native
API correctness at all.

## Documentation

Update:

- top-level and crate Gregg README installation/daemon behavior;
- architecture/gregg-client.md;
- architecture/scripts-and-packaging.md;
- .opencode/skills/gregg-client/SKILL.md;
- .opencode/skills/release-process/SKILL.md;
- AGENTS.md;
- CHANGELOG.md;
- installer help text.

## Acceptance criteria

- [ ] Bare gregg ensures one compatible config-specific client daemon.
- [ ] Concurrent bare launches cannot create duplicate daemons.
- [ ] Daemon identity is verified by local protocol, not PID/process name.
- [ ] The daemon remains running after all TUIs exit.
- [ ] User-scoped startup install/instructions are available on supported OSes.
- [ ] Linux/macOS client startup is not a privileged system service.
- [ ] Windows client startup is user-scoped, not greggd's LocalService SCM.
- [ ] User-local installer can register default client startup safely.
- [ ] System-wide binary install does not guess a user daemon owner.
- [ ] Update prepares/verifies before quiescing the owned daemon.
- [ ] Update restores selected running intent and handles partial success.
- [ ] Protocol-incompatible owned daemon is reconciled on attach.
- [ ] Uninstall stops/removes only owned matching daemon/startup artifacts.
- [ ] Config preservation and --purge semantics remain bounded.
- [ ] No internal sudo or global daemon registry is added.
- [ ] Existing update/install/uninstall regressions remain green.
- [ ] Default local checks and relevant existing native CI jobs pass.

## Stop conditions

Open a corrective plan rather than weaken ownership if:

- a platform cannot provide a user-scoped startup mechanism without admin
  privilege;
- exact artifact ownership cannot be determined;
- update requires killing foreign/incompatible processes by name;
- reliable lazy launch requires a persistent PID registry;
- system-wide installation would need to choose a user account implicitly.

## Handoff

Plan 166 can now assume the client daemon survives outside a TUI session and can
retain a longer in-memory scheduler history. Plan 167 owns measured
multi-client/lifecycle closure.
