# Phase 40: Windows client portability

## Objective

Make the `gregg` client a correct native Windows application before adding the Windows daemon.

The client must build and run in Windows Terminal, store configuration in an appropriate user-scoped location, serialize concurrent configuration mutations across processes, persist edits safely, launch a usable editor, poll existing Linux/macOS daemons, and restore the terminal correctly on every exit path.

This phase deliberately excludes Windows metric collection and Windows service support. A Windows user should be able to install `gregg`, configure remote endpoints, and use the TUI against existing Gregg daemons after this phase.

## Dependency and execution position

Depends on Phase 37 removing obsolete release machinery.

Should use the local validation conventions established by Phase 38.

Must complete before:

- Phase 41 finalizes protocol-v2 client negotiation/rendering;
- Phase 44 declares Windows client support and adds final native CI closure.

Phase 41 may begin after the Windows build/config surfaces are stable.

## Governing invariants

1. Windows support is native and does not require WSL, MSYS2, Cygwin, or Git Bash.
2. The client config is user-scoped, not stored in the current working directory by default.
3. Separate `gregg` processes cannot silently overwrite one another's config mutations.
4. Persistence either completes with a valid full file or leaves the prior valid file intact.
5. Windows-specific failure modes are surfaced as normal typed errors.
6. The TUI uses Windows-supported Crossterm behavior and restores terminal state after errors, Ctrl-C, and panics.
7. Existing Linux/macOS behavior remains unchanged.
8. No release automation is added to prove Windows support.
9. Unsafe Windows API usage, if required, is confined to a narrow module.
10. The phase ends with a usable Windows client even though Windows daemons arrive later.

## Scope

### In scope

- Windows target compilation for `gregg` and `gregg-protocol`;
- user config path resolution;
- cross-process file locking;
- atomic config persistence behavior;
- executable/editor discovery;
- Windows Terminal/Crossterm event behavior;
- Ctrl-C and cleanup behavior;
- endpoint parsing/polling on Windows;
- path and error diagnostics;
- Windows-specific tests and a short native manual smoke;
- documentation of Windows client-only support.

### Out of scope

- compiling or running `greggd` on Windows;
- Windows metrics APIs;
- protocol-v2 metric semantics beyond temporary compatibility scaffolding;
- Windows service control;
- installer/package-manager distribution;
- registry-backed configuration;
- DPAPI or secret storage, because Gregg config contains endpoints rather than credentials;
- PowerShell remoting or remote command execution;
- Windows ARM64 support claims;
- automated releases.

## Workstream A: establish a Windows build baseline

Add a Windows-native build/test check for the client crates, initially as a local/manual command and then in the simplified CI workflow when appropriate:

```powershell
cargo check -p gregg-protocol --all-features
cargo check -p gregg --all-targets --all-features
cargo test -p gregg-protocol --all-features
cargo test -p gregg --all-targets --all-features
```

Do not require `greggd` to compile yet. If workspace-wide commands currently force the daemon to compile, introduce a temporary documented package selection for this phase rather than adding a fake Windows daemon collector.

Inventory failures by category:

- Unix-only imports or APIs;
- target dependencies;
- path assumptions;
- process/executable lookup;
- tests that assume Unix permissions or `flock`;
- terminal tests that require a TTY;
- shell-script-only test entry points.

### Workstream A acceptance criteria

- [ ] `gregg-protocol` compiles and tests on native Windows.
- [ ] `gregg` compiles and tests on native Windows.
- [ ] No unsupported daemon fallback is introduced merely to make `--workspace` green.
- [ ] Windows-specific failures are recorded in implementation notes before being fixed.

## Workstream B: implement a correct Windows config path

The current generic fallback to `gregg.toml` in the working directory is not acceptable for a supported Windows client.

Target default:

```text
%APPDATA%\gregg\gregg.toml
```

Use a dedicated path-resolution function with testable environment access. Preferred resolution order:

1. roaming application-data known folder or `APPDATA` equivalent;
2. a deterministic user-profile fallback such as `%USERPROFILE%\AppData\Roaming` when the primary source is unavailable;
3. a typed error if no user-scoped base directory can be determined.

Do not silently fall back to the current working directory for normal supported Windows execution.

The explicit `--config PATH` option continues to override the default.

### Required tests

- `APPDATA`/known-folder success;
- path with spaces and non-ASCII characters;
- missing primary variable with valid user-profile fallback;
- all user-directory sources unavailable returns a clear error;
- explicit config path bypasses default resolution;
- parent directory creation;
- no accidental Unix separator assumptions.

### Workstream B acceptance criteria

- [ ] Default Windows config is user-scoped.
- [ ] Path resolution is deterministic and testable.
- [ ] Current-directory fallback is removed for supported Windows.
- [ ] Paths with spaces and Unicode work.
- [ ] Linux/macOS paths remain unchanged.

## Workstream C: add real Windows cross-process config locking

The current non-Unix fallback uses only an in-process mutex, which does not protect against two separate client processes.

Implement one of these narrow strategies, in preference order:

1. replace the platform-specific `flock` implementation with a small cross-platform file-lock dependency whose Windows implementation uses native file locking and whose MSRV is verified;
2. implement a contained Windows `LockFileEx`/`UnlockFileEx` adapter behind the existing `FileLockGuard` contract;
3. use Windows file sharing modes to hold an exclusive lock file handle, provided tests prove correct contention and release semantics.

Do not use lock-file existence as the lock. A stale file must not imply a stale lock.

### Required semantics

- bounded acquisition timeout remains approximately five seconds unless deliberately revised;
- retry/backoff does not busy-spin;
- lock is released when the guard drops;
- a crashed process releases the OS lock;
- lock file may persist harmlessly;
- separate processes serialize read-modify-write transactions;
- timeout reports the lock path and duration;
- access denied is distinct from timeout where possible.

### Required tests

Unit tests:

- uncontended acquisition;
- second acquisition blocks/retries then times out;
- dropping first guard permits acquisition;
- lock file persistence does not block later acquisition;
- error formatting.

Native process test:

- process A acquires lock and signals readiness;
- process B attempts mutation and cannot commit while A holds it;
- after A releases, B completes;
- final config is valid and contains one serialized result rather than truncated/interleaved content.

Use a small test helper binary or child-process test mode; do not infer cross-process behavior from two threads in one process.

### Workstream C acceptance criteria

- [ ] Windows mutations have an OS-backed cross-process lock.
- [ ] Lock acquisition is bounded and non-busy.
- [ ] Crash/drop release semantics are proven natively.
- [ ] Concurrent process mutation cannot corrupt config.
- [ ] Unix locking behavior remains correct.

## Workstream D: prove Windows atomic persistence semantics

Review `Config::write_atomic` and transaction editing under Windows.

Required sequence:

1. create a unique temporary file in the destination directory;
2. write complete bytes;
3. flush userspace buffers;
4. close the temporary file before replacement;
5. replace the destination using a Windows-compatible operation;
6. reopen and parse the final file;
7. clean temporary files on failure;
8. retain the prior valid file if replacement fails before commit.

If `std::fs::rename` provides the required tested behavior for the supported Windows baseline, keep it. If ACL preservation, antivirus sharing, or existing-file replacement tests expose gaps, add a contained replacement adapter using an appropriate Windows API. Do not add complex journaling.

### Required Windows tests

- create new config;
- replace existing config;
- repeated mutation;
- destination path with spaces/Unicode;
- destination held open without delete sharing causes a clear failure and preserves old content;
- temporary file cleanup after write failure;
- temporary file cleanup after replacement failure;
- final config reparses exactly;
- concurrent reader observes either old or new complete TOML, never partial content;
- edit transaction validation failure preserves original file.

Windows does not expose Unix permission bits. Do not add meaningless `0600` assertions. Document that config contains no credentials and relies on the user's profile-directory ACL by default.

### Workstream D acceptance criteria

- [ ] Existing-file replacement works on supported Windows.
- [ ] Sharing violations fail safely.
- [ ] No partial TOML is exposed.
- [ ] Temporary files are cleaned when possible.
- [ ] Edit validation remains transactional.
- [ ] Unix durability behavior is not weakened.

## Workstream E: make editor discovery portable

The current fallback uses the Unix `which` command and Unix editor names.

Refactor executable discovery into a platform-neutral helper.

Resolution order remains:

1. `$VISUAL`;
2. `$EDITOR`;
3. platform fallback list.

Windows fallback suggestions:

```text
hx.exe
code.cmd or code.exe when present
notepad.exe
```

Do not require VS Code. `notepad.exe` is the final ubiquitous fallback.

Executable lookup must honor:

- `PATH` entries;
- `PATHEXT` for extensionless command names;
- quoted environment values only if parsing is deliberately supported;
- paths with spaces;
- direct absolute paths.

The existing behavior treats `$VISUAL`/`$EDITOR` as one executable string and does not safely parse argument lists. Preserve that simple contract unless a small, well-tested command-spec parser is deliberately added. Document that variables should identify an executable path rather than an arbitrary shell command.

### Required tests

- explicit absolute editor path;
- executable found via PATH/PATHEXT;
- missing editor falls back to Notepad;
- path with spaces;
- nonexistent environment-selected editor returns a useful launch error;
- no invocation of `which` on Windows;
- editor exit failure preserves original config;
- successful edit validates and commits.

### Workstream E acceptance criteria

- [ ] Windows editor resolution requires no Unix command.
- [ ] Notepad fallback works.
- [ ] Edit remains transactional.
- [ ] Paths with spaces work.
- [ ] Linux/macOS fallback behavior remains available.

## Workstream F: validate polling and endpoint behavior on Windows

The networking stack should be portable, but prove it natively.

Test:

- IPv4 loopback endpoint;
- bracketed IPv6 loopback endpoint;
- DNS hostname;
- timeout behavior;
- connection refused behavior;
- malformed URL/endpoint rejection;
- bounded concurrent polling;
- cancellation during in-flight requests;
- HTTP response size/validation limits already enforced by the client;
- v1 Linux/macOS fixture parsing.

Use loopback test servers and deterministic fixtures. Do not depend on a LAN daemon in CI.

Ensure endpoint display formatting uses Windows-independent string rules and does not rely on Unix socket concepts.

### Workstream F acceptance criteria

- [ ] Polling works against loopback IPv4 and IPv6 on Windows.
- [ ] Cancellation and timeout behavior are deterministic.
- [ ] Existing protocol fixtures parse identically.
- [ ] No Unix-specific network assumptions remain in client code.

## Workstream G: validate TUI and terminal lifecycle

Crossterm supports Windows, but terminal behavior must be exercised in Windows Terminal or a compatible console host.

Review:

- raw-mode enable/disable;
- alternate screen entry/exit;
- cursor hide/show;
- event-stream shutdown;
- resize events;
- Ctrl-C cancellation;
- panic hook restoration;
- stdout/stderr diagnostics before/after terminal mode;
- Unicode width/rendering behavior;
- arrow keys and `j`/`k` navigation.

### Automated tests

Keep terminal-independent UI layout/rendering tests. TTY-dependent tests should skip truthfully when no console is attached rather than fail or claim coverage.

Add test seams where necessary so restoration calls can be validated without requiring a live terminal.

### Native manual smoke

Document a short Windows Terminal check:

1. `cargo run -p gregg -- list`;
2. add a loopback fixture endpoint;
3. launch TUI;
4. resize window;
5. navigate;
6. press Ctrl-C and `q` in separate runs;
7. trigger a controlled fixture disconnect;
8. confirm terminal is restored after normal exit and forced fixture failure.

### Workstream G acceptance criteria

- [ ] TUI starts in Windows Terminal.
- [ ] Keyboard and resize events work.
- [ ] Normal exit, Ctrl-C, and panic/error paths restore the terminal.
- [ ] Noninteractive tests skip rather than fabricate TTY success.
- [ ] Unicode/path diagnostics render acceptably.

## Workstream H: error model and documentation

Update errors so Windows-specific failures remain actionable:

- config directory unavailable;
- lock access denied;
- lock timeout;
- sharing violation during replacement;
- editor not found/failed;
- terminal initialization failed;
- endpoint connection/timeout failure.

Do not expose raw numeric Windows error codes without a human-readable OS error.

Update README support table to distinguish:

```text
Windows x86-64 client: supported after Phase 40
Windows daemon: not yet supported; planned in Phases 41-43
```

Add Windows config path and editor notes.

Do not claim Windows service or local Windows metrics support yet.

### Workstream H acceptance criteria

- [ ] Error messages identify operation and path/endpoint.
- [ ] README accurately states client-only Windows support.
- [ ] Windows config location is documented.
- [ ] No premature Windows daemon claim is made.

## Required validation commands

On Windows PowerShell:

```powershell
cargo fmt --all -- --check
cargo clippy -p gregg-protocol -p gregg --all-targets --all-features -- -D warnings
cargo test -p gregg-protocol -p gregg --all-targets --all-features
cargo doc -p gregg-protocol -p gregg --no-deps
cargo build -p gregg --release
cargo run -p gregg -- --help
cargo run -p gregg -- list
```

Run the native cross-process lock test and Windows Terminal smoke.

On Linux/macOS, run the normal full local validation to prove no regression.

## Phase acceptance criteria

Phase 40 is complete only when:

- [ ] `gregg-protocol` and `gregg` compile and test natively on Windows x86-64.
- [ ] Windows client config defaults to a user-scoped application-data path.
- [ ] Cross-process config mutation uses a real Windows OS lock.
- [ ] Atomic replacement and sharing-violation cases are tested natively.
- [ ] Editor discovery does not invoke Unix `which` and has a Notepad fallback.
- [ ] Polling, timeout, cancellation, IPv4, and IPv6 loopback behavior pass on Windows.
- [ ] TUI input, resize, exit, and terminal restoration work in Windows Terminal.
- [ ] Existing Linux/macOS client behavior remains green.
- [ ] README accurately advertises Windows client-only support.
- [ ] No Windows daemon/service claim or release automation is introduced.

## Evidence required for completion

Only:

- passing native Windows test output;
- one concise manual Windows Terminal smoke note;
- passing Linux/macOS local validation;
- code and documentation diff.

Do not upload a special evidence artifact or create a release qualification workflow.

## Handoff notes for a smaller implementation model

1. Get `gregg` and `gregg-protocol` compiling on Windows before changing behavior.
2. Implement config path and locking as separate commits; both affect persistence but have distinct failure modes.
3. Use a real child-process test for locking.
4. Do not rely on lock-file existence.
5. Drop file handles before replacement tests.
6. Refactor editor lookup into a small standalone helper with deterministic tests.
7. Keep TTY-dependent validation manual or truthfully skipped in CI.
8. Update support claims only after native smoke completion.
9. Do not touch protocol metric semantics beyond what the existing client needs; Phase 41 owns that redesign.