# Plan 130: user-local installer PATH activation and shell persistence

Status: planned.

Depends on: the completed installer ownership work in Plans 112-116 and the current post-Plan-129 repository baseline. This work is independent of the remaining Plan 091 soak record.

## Objective

Close the user-visible gap between a successful non-root `gregg` bootstrap install and immediate command usability.

The current Unix bootstrap deliberately installs non-root binaries to `$HOME/.local/bin`, but when that directory is not already in the invoking shell's `PATH` it only prints advice. The documented `gregg` install is non-root, while the documented `greggd` install normally runs under `sudo` and lands in `/usr/local/bin`, which is usually already on `PATH`. That makes `greggd` appear immediately available while a fresh `gregg` install can end with `gregg: command not found`.

Preserve the existing install-scope contract:

```text
root      -> /usr/local/bin
non-root  -> $HOME/.local/bin
```

Do not move the user-local client into a system directory, search arbitrary `PATH` locations for an alternate destination, silently invoke `sudo`, or add an install receipt merely to solve command activation.

The bootstrap should instead make the standard user-local path persistent for supported interactive shells when it is missing, report exactly what it changed, and document a one-line install form whose trailing `export` executes in the invoking shell so the newly installed command is usable immediately without restarting the terminal.

## Current behavior and constraint

`packaging/install.sh` currently calls `check_path_advice()` after a successful install. If `${DEST_DIR}` is absent from `PATH`, it prints:

```text
note: $HOME/.local/bin is not in PATH; add it to your shell profile:
  export PATH="$HOME/.local/bin:$PATH"
```

It does not modify shell startup files.

That behavior is truthful but leaves a successful user-local bootstrap in a partially activated state.

There is also a hard process-boundary constraint: an installer invoked as:

```bash
curl .../install.sh | bash -s -- gregg
```

runs in a child shell and cannot mutate the environment of its parent interactive shell. Running `export PATH=...` inside the installer cannot make `gregg` resolvable by the already-running parent after the installer exits.

Do not pretend otherwise.

For immediate same-shell activation, the documented rootless form may use the invoking shell itself after the pipeline succeeds:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | bash -s -- gregg \
  && export PATH="$HOME/.local/bin:$PATH"
```

The installer-owned shell-profile integration provides persistence for later shells; the trailing parent-shell `export` provides immediate usability in the current shell.

## Required implementation

### 1. Preserve canonical destinations and same-scope ownership

Do not change the Plan-112 destination rule or replacement classification.

The bootstrap must continue to install:

- root invocations to `/usr/local/bin`;
- non-root invocations to `$HOME/.local/bin`;
- only the selected `gregg`, `greggd`, or both components;
- same-scope replacements in place;
- no arbitrary global/user install discovery.

PATH work begins only after the selected candidate has been verified and installed successfully.

Do not make shell-profile mutation part of candidate verification, download, Cargo fallback, daemon ownership, update, or uninstall identity.

### 2. Replace advisory-only PATH handling with bounded user-shell integration

For a non-root install, when `$HOME/.local/bin` is absent from the invoking process's `PATH`, attempt a bounded, idempotent shell-profile update.

At minimum support the two primary Unix interactive shells used by Gregg's target hosts:

- zsh: user interactive startup file (`${ZDOTDIR:-$HOME}/.zshrc`, honoring an exported safe `ZDOTDIR` when practical);
- bash: choose the normal interactive startup target for the host, with Linux centered on `~/.bashrc` and macOS login-shell behavior handled deliberately rather than accidentally.

Implementation review may support additional shells only when the behavior is deterministic and testable. Unknown shells must fall back to precise manual advice rather than guessing a startup file.

The persisted entry must be shell-appropriate and equivalent to prepending the canonical user-local bin directory:

```text
$HOME/.local/bin
```

Do not persist an expanded absolute home path when a stable `$HOME` expression is available.

### 3. Keep profile mutation idempotent and minimally invasive

The installer must not append the same PATH entry on every rerun.

Before writing:

- detect an existing Gregg-managed entry;
- conservatively recognize an existing user-authored startup line that already references `$HOME/.local/bin` or `~/.local/bin` and avoid adding a redundant entry;
- preserve the rest of the startup file byte-for-byte except for the bounded appended integration;
- create a missing ordinary startup file only when its parent is the user's expected home/config location;
- never truncate or rewrite the whole profile;
- never evaluate profile contents;
- never source the profile from the installer.

Use a small recognizable comment/managed marker if needed for idempotency, but do not create a broad shell-configuration subsystem.

A malformed, inaccessible, or unsupported profile must not roll back an already successful binary install. Report the PATH integration failure separately and print the exact manual command required.

### 4. Add an explicit opt-out

Add a narrow bootstrap option such as:

```text
--no-shell-profile
```

for users who manage dotfiles or PATH externally.

The flag must:

- suppress all shell startup-file mutation;
- leave binary destination and installation semantics unchanged;
- still report when the destination is absent from current `PATH`;
- print the immediate/manual `export PATH="$HOME/.local/bin:$PATH"` guidance.

Do not make this a generic "no configuration" mode; it controls shell-profile PATH integration only.

### 5. Do not touch profiles for system installs

Root/system installation to `/usr/local/bin` must not edit the invoking user's or root's shell profile.

If `/usr/local/bin` is unexpectedly absent from the root invocation's `PATH`, retain bounded advice only. Do not attempt to repair system-wide shell policy, `/etc/paths`, `/etc/profile`, `/etc/zprofile`, or other global startup configuration.

This keeps `greggd` service installation and system bootstrap behavior independent from interactive-shell preferences.

### 6. Make output distinguish current-shell and future-shell state

Successful user-local output must distinguish three cases.

If the destination was already in `PATH`:

```text
Installed gregg to $HOME/.local/bin/gregg
gregg is available on the current PATH.
```

If a profile entry was added but the invoking process did not already have the destination:

```text
Installed gregg to $HOME/.local/bin/gregg
Added $HOME/.local/bin to <profile> for future shells.
For this shell, run:
  export PATH="$HOME/.local/bin:$PATH"
```

If profile integration was skipped/unsupported/failed:

```text
Installed gregg to $HOME/.local/bin/gregg
$HOME/.local/bin is not on the current PATH.
Add it to your shell configuration, or run for this shell:
  export PATH="$HOME/.local/bin:$PATH"
```

Do not print "ready to use" when command-name resolution has not actually been established for the current process.

For `both`, perform PATH integration once, not once per component.

### 7. Update the documented quick-install path for immediate usability

Update the user-facing installation examples so the ordinary rootless client path has a copy/paste form that works in the same shell without a terminal restart:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | bash -s -- gregg \
  && export PATH="$HOME/.local/bin:$PATH"
```

Explain why the trailing `export` is outside the installer pipeline: it runs in the invoking shell.

Keep the shorter existing pipeline documented as valid when `~/.local/bin` is already on PATH or when the operator is content to open a later shell after profile persistence.

Do not change the recommended privileged `greggd` command merely to create visual symmetry; its immediate availability already follows from installation into `/usr/local/bin`.

### 8. Keep uninstall ownership unchanged

Do not make `gregg uninstall` or `greggd uninstall` delete the generic `$HOME/.local/bin` PATH entry.

That directory is a standard user executable location and may contain unrelated tools. Removing its shell-profile entry because one Gregg component was uninstalled could break other software or a sibling Gregg component.

Document this explicitly. The profile entry is user-environment integration, not component-owned binary state.

Do not add an install receipt or per-profile mutation ledger in this phase.

## Deterministic verification

Extend the existing Unix bootstrap harness rather than adding a new workflow.

Use isolated temporary `HOME`, `PATH`, `SHELL`, and profile files in `scripts/tests/test-install-rerun.sh` (or a narrowly split companion if the existing harness becomes unwieldy). Never mutate the test runner's real profile.

Cover at least:

- non-root zsh-style install with `~/.local/bin` absent from PATH -> profile entry added;
- rerun -> no duplicate entry;
- existing user-authored `.local/bin` profile entry -> no redundant Gregg entry;
- supported bash behavior on Linux;
- macOS bash profile-target selection through deterministic helper inputs rather than requiring a real interactive shell;
- `--no-shell-profile` -> zero profile mutation plus exact manual guidance;
- unsupported shell -> zero profile mutation plus exact manual guidance;
- destination already in current PATH -> no unnecessary profile mutation;
- root/system destination -> no user profile mutation;
- missing profile file -> bounded creation at the selected user path;
- inaccessible/invalid profile target -> binary install remains successful and output reports activation failure truthfully;
- `both` -> one PATH integration action;
- first install and same-scope replacement behavior from Plan 112 remains unchanged.

Where practical, factor shell/profile selection and "already contains local-bin" detection into helpers that can be exercised without downloads.

Retain the existing fake curl/Cargo strategy; no network access is needed for these regressions.

## Standard gates

Run:

```text
bash scripts/tests/test-install-rerun.sh
cargo test -p greggd --test installer_rerun
./scripts/check-local.sh
```

Then use the existing ordinary CI workflow at the final implementation SHA.

No new workflow, matrix, privileged shell-profile smoke, self-hosted runner, or release publication is required.

Because this is shell/packaging behavior, ShellCheck coverage must remain clean for `packaging/install.sh` and the installer test harness.

## Documentation and architecture reconciliation

Update the affected current documentation in the same implementation pass:

- `README.md`;
- `docs/installation.md`;
- `packaging/README.md`;
- `architecture/scripts-and-packaging.md`;
- `CHANGELOG.md`;
- `.opencode/skills/release-process/SKILL.md` if its install examples or installer contract mention advisory-only PATH behavior.

The old statement that the Unix bootstrap "never edits shell rc files" becomes historical/outdated once this plan lands and must be replaced with the narrower truth: only non-root user-local PATH persistence may make a bounded user-profile edit; system installs and unrelated shell configuration remain untouched.

Do not rewrite closed Plan 112's historical record. Plan 130 supersedes only its current advisory-only PATH behavior.

## Acceptance criteria

- [ ] Non-root `gregg` installation still uses `$HOME/.local/bin`; root installation still uses `/usr/local/bin`.
- [ ] Same-scope replacement, foreign-destination refusal, staged Cargo fallback, daemon finalization, update, and uninstall ownership semantics from Plans 112-116 remain unchanged.
- [ ] When `$HOME/.local/bin` is absent from current PATH, supported user shells receive a bounded persistent PATH integration by default.
- [ ] zsh and bash profile selection are deterministic, documented, and covered without touching the operator's real dotfiles.
- [ ] Profile integration is idempotent across repeated installer runs.
- [ ] An existing user-authored `~/.local/bin` PATH entry is not redundantly duplicated.
- [ ] `--no-shell-profile` suppresses all startup-file mutation while preserving successful installation and precise manual guidance.
- [ ] Unknown/unsupported shells do not guess a profile file and do not turn a successful binary install into a failure.
- [ ] Root/system installs never mutate user or global shell startup files.
- [ ] Profile-write failure is reported separately and does not erase or misreport the successful binary installation.
- [ ] Installer output distinguishes current-shell availability from persistence for future shells.
- [ ] The documented client quick-install form includes a parent-shell trailing `export` path that makes `gregg` immediately resolvable after a successful install without restarting the terminal.
- [ ] Documentation explicitly states that a piped child installer cannot directly change its parent's environment.
- [ ] `both` performs at most one PATH/profile integration action.
- [ ] Uninstall does not remove the generic `$HOME/.local/bin` PATH integration.
- [ ] Installer/profile regressions run through the existing deterministic harness and ordinary CI; no new workflow or privileged profile smoke is introduced.
- [ ] Current docs and architecture no longer claim that user-local Unix bootstrap categorically never edits shell rc files.
- [ ] Plan 130 closure records the implementation SHA and exact CI run used.

## Explicit non-goals

Do not include:

- moving ordinary non-root `gregg` installation to `/usr/local/bin`;
- silently invoking or recommending mandatory `sudo` for the client;
- selecting arbitrary writable directories from the user's PATH as install destinations;
- creating PATH launcher symlinks in Homebrew or other tool-managed directories;
- editing `/etc/profile`, `/etc/paths`, `/etc/paths.d`, `/etc/zprofile`, or other system shell policy;
- sourcing or evaluating user startup files from the installer;
- forcing a shell restart or `exec $SHELL`;
- pretending a child process can export environment variables into its parent;
- a general-purpose dotfile manager;
- an install receipt solely to track shell-profile edits;
- removing generic user PATH configuration during Gregg uninstall;
- Windows PATH redesign;
- release workflow changes.

## Handoff note

Start in `packaging/install.sh` and `scripts/tests/test-install-rerun.sh`.

Keep the ownership boundary simple: the binary still has one canonical destination per privilege scope. PATH integration is a post-install user-experience step, not installation identity.

The most important correctness rule is to separate persistence from current-process activation. Editing `.zshrc` or a bash profile helps later shells; it does not change the already-running parent that launched a piped installer. The quick-install documentation should therefore use a trailing parent-shell `&& export PATH="$HOME/.local/bin:$PATH"` when immediate same-session command resolution is desired.
