# Plan 131: installer PATH profile detection corrective pass

Status: planned.

Depends on: completed Plan 130 and its settled user-local PATH persistence/activation contract. This work is independent of the remaining Plan 091 soak record.

## Objective

Correct one narrow post-Plan-130 installer defect in the existing-profile detector without reopening installer ownership, shell-profile selection, current-shell activation, update/uninstall semantics, or release architecture.

Plan 130 added bounded, idempotent PATH persistence for supported user shells. Its current `profile_contains_local_bin()` implementation treats any textual occurrence of `.local/bin` anywhere in the selected startup file as proof that the profile already activates `$HOME/.local/bin`:

```bash
if grep -Fq ".local/bin" "$file" 2>/dev/null; then
  return 0
fi
```

That is too broad. A commented-out PATH line, documentation/comment text, an alias/echo, or an unrelated pathname containing `.local/bin` can suppress the Gregg-managed PATH block even though a future shell will not actually place `$HOME/.local/bin` on `PATH`.

The corrective pass must make "already integrated" mean a bounded, plausibly active PATH integration rather than an arbitrary substring match.

## Post-Plan-130 finding

The false-positive behavior is directly reproducible with inputs such as:

```bash
# export PATH="$HOME/.local/bin:$PATH"
```

or:

```bash
# My local tools live under ~/.local/bin
```

or an unrelated active line such as:

```bash
echo "$HOME/.local/bin"
```

Under the current detector, each contains the literal substring `.local/bin`, so the installer reports that the shell profile already references the destination and skips persistence.

This does not invalidate Plan 130's canonical destination rule, profile-selection policy, append-only managed block, `--no-shell-profile`, system-install exclusion, current-shell guidance, `both` single-integration behavior, or uninstall ownership. It is a classification defect at the "existing profile already integrates local bin" boundary.

## Required implementation

### 1. Replace substring presence with an active-integration predicate

Replace or rename `profile_contains_local_bin()` so its contract is explicit: it must return success only when the selected startup file contains either:

1. Gregg's exact managed PATH integration marker/block in an intact recognizable form; or
2. a supported, non-commented user-authored shell expression that plausibly mutates `PATH` (or zsh's tied `path` array) to include the canonical user-local bin directory.

Do not treat an arbitrary `.local/bin` occurrence as sufficient.

The helper must remain read-only. Never source, evaluate, execute, command-substitute, or otherwise interpret profile contents as shell code.

### 2. Recognize the common active user-authored forms conservatively

At minimum, deterministic recognition must cover ordinary active PATH assignment forms already implied by Plan 130, including equivalent whitespace/quoting variations of:

```bash
export PATH="$HOME/.local/bin:$PATH"
PATH="$HOME/.local/bin:$PATH"
export PATH="${HOME}/.local/bin:${PATH}"
export PATH="$PATH:$HOME/.local/bin"
```

Recognize `~/.local/bin` where it is used in a plausibly active PATH assignment.

For zsh, implementation may also recognize the common tied-array form when it is simple and unambiguous, for example:

```zsh
path=($HOME/.local/bin $path)
```

Do not attempt to implement a general shell parser. False negatives on unusual shell metaprogramming are preferable to false positives that leave a successful install non-persistent. If a user uses an exotic construct the installer may append Gregg's idempotent managed block.

### 3. Ignore comments and unrelated references

The detector must not suppress integration for:

- a full-line comment containing `.local/bin`;
- a commented-out former PATH assignment;
- prose/documentation strings;
- `echo` / `printf` / aliases / functions that merely mention the path;
- unrelated variables containing a `.local/bin` pathname;
- other paths that merely contain the same substring.

Leading whitespace before a comment must still classify the line as commented.

Inline comments on an otherwise active recognized assignment may be accepted if recognition remains deterministic and does not require shell evaluation.

### 4. Keep Gregg-managed idempotency stable

A profile written by the current Plan-130 installer must still be recognized on rerun so the installer never appends a second managed block.

Prefer an exact Gregg marker plus a recognizable managed-block shape rather than a generic substring test.

If implementation review discovers that the current marker alone can survive while its functional PATH line has been manually disabled, define the smallest truthful rule and test it. Do not make the detector claim future-shell persistence solely because an unrelated comment happens to match the marker text.

Do not rewrite or normalize an existing valid managed block merely to adopt the corrected detector.

### 5. Preserve all Plan-130 boundaries

Do not change:

- root `/usr/local/bin` versus non-root `$HOME/.local/bin` destinations;
- same-scope replacement or foreign-destination refusal;
- binary download/checksum/version verification;
- Cargo fallback staging;
- zsh/bash profile target selection;
- safe `ZDOTDIR` handling;
- `--no-shell-profile`;
- unsupported-shell fallback;
- system-install no-profile-mutation behavior;
- current-parent-shell `export PATH="$HOME/.local/bin:$PATH"` guidance;
- `both` integrating at most once;
- `gregg update`, `greggd update`, or daemon lifecycle behavior;
- uninstall preserving the generic local-bin profile integration;
- Windows installer/PATH behavior;
- CI/release workflow topology.

This pass owns only existing-profile classification plus the tests/docs needed to state that classification truthfully.

## Deterministic verification

Extend the existing isolated Plan-130 section of `scripts/tests/test-install-rerun.sh`.

At minimum add regressions proving:

- active `export PATH="$HOME/.local/bin:$PATH"` suppresses a duplicate managed block;
- active `PATH="$HOME/.local/bin:$PATH"` suppresses a duplicate managed block;
- an active append form such as `export PATH="$PATH:$HOME/.local/bin"` suppresses a duplicate managed block;
- a simple supported `~/.local/bin` PATH assignment suppresses duplication;
- the exact Gregg-managed block remains idempotent on rerun;
- `# export PATH="$HOME/.local/bin:$PATH"` does **not** suppress Gregg integration;
- a whitespace-indented commented PATH assignment does not suppress integration;
- a prose comment mentioning `~/.local/bin` does not suppress integration;
- `echo "$HOME/.local/bin"` does not suppress integration;
- an unrelated variable assignment such as `TOOLS="$HOME/.local/bin/tool"` does not suppress integration;
- unrelated profile content remains byte-for-byte preserved before the appended managed block;
- the resulting output truthfully reports a newly added future-shell integration for false-positive fixtures.

Keep all test HOME/PATH/SHELL/profile state isolated. No test may mutate the runner's real startup files.

The existing Plan-130 tests must continue to pass unchanged unless a test was explicitly encoding the over-broad substring behavior; in that case tighten the fixture while preserving the original behavioral intent.

## Standard gates

Run:

```text
bash scripts/tests/test-install-rerun.sh
cargo test -p greggd --test installer_rerun
./scripts/check-local.sh
```

Keep ShellCheck clean for `packaging/install.sh` and the installer harness.

Then use the existing ordinary CI workflow at the final implementation SHA. No new workflow, job, matrix, privileged profile smoke, self-hosted runner, or release publication is required.

## Documentation and record reconciliation

User-facing installation semantics do not materially change: Plan 130 already documents that active existing user PATH integration is not duplicated.

Update only documentation/comments that currently imply **any reference** to `.local/bin` is enough. The truthful rule after this pass is that a recognizable active PATH integration or intact Gregg-managed integration suppresses a duplicate block.

Append a post-closure correction note to Plan 130 rather than rewriting its implementation SHA, CI evidence, checked acceptance list, or original closure narrative. The note must state that:

- post-closure review found the over-broad substring classifier;
- comments/unrelated references can falsely suppress persistence;
- Plan 131 owns the detector/test correction;
- Plan 130's destination, profile-selection, activation, opt-out, system-install, and ownership work remains valid.

When Plan 131 closes, update `plans/README.md` to show Plan 130 complete with a Plan-131 corrective follow-up and Plan 131 complete with its implementation/CI evidence.

## Acceptance criteria

- [ ] Existing-profile classification no longer treats arbitrary `.local/bin` text as proof of PATH integration.
- [ ] Full-line and leading-whitespace comments containing `.local/bin` cannot suppress the managed block.
- [ ] A commented-out former PATH assignment cannot suppress integration.
- [ ] Unrelated `echo`, prose, alias/function text, or non-PATH variable references cannot suppress integration.
- [ ] Common active user-authored PATH assignment/prepend/append forms containing `$HOME/.local/bin`, `${HOME}/.local/bin`, or `~/.local/bin` remain recognized without duplication.
- [ ] The existing Gregg-managed block remains idempotently recognized on rerun.
- [ ] Detection remains static/read-only: profile contents are never sourced, evaluated, executed, or command-substituted.
- [ ] Unusual/unrecognized shell metaprogramming fails conservative: the installer may append its safe managed block rather than falsely claiming persistence.
- [ ] Existing profile content is preserved except for the same bounded append already introduced by Plan 130.
- [ ] False-positive fixtures now produce truthful `Added ... for future shells` output and the exact current-shell export guidance.
- [ ] All existing Plan-130 destination, profile-selection, opt-out, system-install, `both`, update/uninstall, and platform boundaries remain unchanged.
- [ ] Deterministic installer regressions and the ordinary local/CI gates pass without new infrastructure.
- [ ] Plan 130 receives an appended post-closure correction note rather than rewritten historical closure evidence.
- [ ] Plan 131 closure records the implementation SHA and exact CI run used.

## Explicit non-goals

Do not include:

- a general bash/zsh parser;
- sourcing or evaluating startup files to discover effective PATH;
- recursively following shell includes/source statements;
- modeling conditional shell execution;
- resolving arbitrary aliases/functions;
- changing profile target selection;
- adding support for additional shells solely for this fix;
- moving the install destination;
- changing immediate parent-shell activation semantics;
- changing uninstall behavior;
- Windows PATH work;
- install receipts or profile mutation ledgers;
- release/CI architecture changes;
- unrelated installer cleanup.

## Handoff note

Start with `profile_contains_local_bin()` in `packaging/install.sh` and the Plan-130 PATH fixtures in `scripts/tests/test-install-rerun.sh`.

The intended bias is conservative: only suppress Gregg's managed append when there is strong static evidence that the selected profile already provides the canonical user-local bin directory through PATH integration. A harmless duplicate managed block on an exotic configuration is preferable to a false positive that leaves future shells unable to resolve `gregg`.
