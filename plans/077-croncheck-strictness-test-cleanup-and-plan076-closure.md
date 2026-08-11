# Phase 077: croncheck strictness, test cleanup, and Plan 076 closure

Status: planned.

Depends on: Plan 076 implementation (`b17037d`) and follow-up cleanup through `437b77a`.

## Objective

Close the small correctness and maintainability gaps left after Plan 076 without reopening Gregg's Unix runtime architecture.

Plan 076 successfully removed systemd/launchd control from the Unix runtime, converted `croncheck` into an observational HTTP probe, made Unix `host`/`port` config-only operations, and added explicit `version` subcommands. This corrective phase is limited to four remaining items:

1. make `croncheck` status-line reading genuinely bounded;
2. reject premature EOF and malformed HTTP versions/status lines precisely enough for a local health probe;
3. add the missing negative-path regression tests and remove the permanently disabled legacy service-manager test block while retaining still-useful current-contract coverage;
4. reconcile Plan 076 and the plan index after local verification.

This is not a new runtime, protocol, service, or testing architecture. Do not add dependencies, retries, generic HTTP parsing, CI work, or another supervisor abstraction.

## Observed remaining defects

### 1. `croncheck` response reading is not bounded by bytes

Current `probe_health()` uses `BufReader::read_line(&mut String)`. The socket read has a timeout, but the line buffer itself has no fixed byte ceiling. A peer can send an arbitrarily long unterminated first line before the timeout, causing unnecessary allocation in a command intended to be a small local probe.

Plan 076 explicitly required a fixed/bounded response read. This phase must make that requirement true in code rather than relying on the timeout alone.

### 2. Premature EOF can be accepted as healthy

`read_line()` returns accumulated bytes when EOF occurs before a newline. Therefore a peer that writes:

```text
HTTP/1.1 200 OK
```

without the required HTTP line terminator and closes the connection can currently be accepted as healthy.

For this command, an incomplete status line is malformed transport data and must return nonzero.

### 3. HTTP-version validation is looser than intended

The current parser accepts any first token beginning with `HTTP/1.`. The probe does not need a general HTTP parser, but it should recognize only the HTTP/1.x forms Gregg can deliberately support for this status-line check. Accepting arbitrary strings such as `HTTP/1.xyz` is unnecessary.

The implementation should stay conservative and tiny: exact HTTP/1.0 and HTTP/1.1 status-line versions are sufficient unless inspection of the existing server proves another concrete form is required.

### 4. Negative-path coverage is incomplete

The active Plan 076 tests cover:

- HTTP 200 success;
- HTTP 503 failure;
- a plainly malformed first line;
- wildcard bind-address normalization;
- config-only mutation;
- version rendering.

They do not explicitly cover all acceptance cases from Plan 076, particularly:

- EOF before a complete CRLF-terminated status line;
- a status line exceeding the fixed bound;
- invalid `HTTP/1.*` version text;
- connection refusal / closed port returning promptly.

### 5. A large obsolete test module is hidden behind an always-false cfg

`crates/greggd/src/cli.rs` retains an old test module under:

```rust
#[cfg(all(test, any()))]
```

An empty `any()` is always false. The block contains obsolete assumptions such as service-manager-driven `croncheck`, Unix `start`/`stop`/`restart`, `mutate_and_restart`, and `systemctl` error strings.

This dead source does not affect the binary, but it obscures the current contract and preserves stale implementation concepts in a small project. Delete the obsolete tests rather than hiding them.

Do not blindly delete useful regression coverage that is not duplicated elsewhere. Before removal, identify still-relevant tests for config-path intent, config validation, exit-code mapping, and atomic persistence. Retain only the minimal non-duplicated cases by adapting them to current APIs or relying on equivalent existing tests in `config.rs`, `main.rs`, or the active CLI test module.

### 6. Planning records are inconsistent

`plans/README.md` describes Plan 076 as complete, while Plan 076 itself is marked `Status: implemented` and its acceptance checklist remains unchecked despite the implementation and Ubuntu E2E record.

After this corrective phase passes, Plan 076 should be reconciled to a truthful completed state and Plan 077 should be closed directly. Do not create a Plan 078 merely to record closure.

## Authoritative behavior after this phase

The architectural contract from Plan 076 remains unchanged:

```text
Unix:
  greggd run       -> native foreground daemon
  greggd croncheck -> read-only local HTTP health probe
  greggd host/port -> atomic config mutation only

Windows:
  existing SCM lifecycle remains intact
```

`croncheck` must continue to:

- load the same resolved config used by `run`;
- normalize wildcard bind addresses to loopback;
- connect to the configured port with a short fixed timeout;
- request `GET /v2/healthz` over HTTP/1.1;
- exit `0` only for a complete, valid HTTP/1.0 or HTTP/1.1 status line with status code `200`;
- return nonzero for 503 or any other non-200 code;
- return nonzero for connection refusal, timeout, premature EOF, malformed version/status text, or an overlong status line;
- never start, stop, restart, install, enable, or otherwise mutate a process or service.

No response body parsing is required.

## Scope

### In scope

- Replace unbounded `read_line(String)` status-line handling with a fixed-size byte buffer or equivalently hard-bounded reader.
- Require a complete CRLF-terminated first status line.
- Parse only the minimal status-line fields needed for health determination.
- Accept exact supported HTTP/1.x versions and status `200` only.
- Add focused negative-path tests for premature EOF, overlong status line, invalid HTTP version, and refused connection.
- Retain existing 200/503/malformed/wildcard tests.
- Delete the always-false legacy CLI test module.
- Preserve still-useful non-duplicated current-contract tests with minimal adaptation where necessary.
- Run focused tests, workspace-local checks, and one narrow Ubuntu live/dead `croncheck` smoke using the real release binary.
- Reconcile Plan 076 and `plans/README.md` after verification.

### Out of scope

- New HTTP client dependencies (`reqwest`, `hyper` client features, etc.).
- A general-purpose HTTP parser or reusable networking framework.
- HTTP/2, TLS, authentication, redirects, retries, DNS discovery, or remote health semantics.
- Changing `/v2/healthz` or any wire schema.
- Reintroducing systemd/launchd runtime code.
- Self-daemonization, PID files, process discovery, signal-based reload, or supervisor integration.
- Changes to Windows SCM lifecycle behavior.
- Broad test-suite restructuring or repository-wide warning cleanup.
- New test harnesses, evidence bundles, workflow jobs, matrices, or CI gates.
- Moving the Ubuntu smoke into CI.
- Release automation or publication work.

## Expected files

Primary implementation surface:

```text
crates/greggd/src/cli.rs
plans/076-native-runtime-croncheck-and-version-correction.md
plans/077-croncheck-strictness-test-cleanup-and-plan076-closure.md
plans/README.md
```

Potentially touched only if inspection proves a directly related documentation statement is inaccurate:

```text
crates/greggd/README.md
architecture/greggd-daemon.md
```

Do not create a new module solely for a status-line parser unless keeping the helper inside `cli.rs` would materially reduce readability. For this scope, one or two small private helpers are preferred.

## Implementation sequence

### Step 1: lock the malformed/EOF cases with tests first

Extend the existing active `native_tests` in `crates/greggd/src/cli.rs` using the current tiny `TcpListener` helper or a minimally adjusted equivalent.

Add explicit tests for:

1. **Premature EOF**: server writes `HTTP/1.1 200 OK` without `\r\n`, closes, and `probe_health()` must fail.
2. **Overlong status line**: server sends more than the fixed maximum before CRLF; probe must fail without growing a heap buffer in proportion to peer input.
3. **Invalid version**: `HTTP/1.xyz 200 OK\r\n` must fail.
4. **Connection refusal**: obtain an ephemeral loopback port from a temporary `TcpListener`, close it, then probe that address. The call must fail within the fixed timeout rather than hanging.
5. Preserve the existing HTTP 200, HTTP 503, and plainly malformed-line assertions.

Avoid exact full error-string assertions. Test the success/failure contract and, where useful, a stable error category substring such as `premature`, `too long`, `malformed`, or `connection`.

Do not add sleeps except where a bounded local socket test cannot otherwise synchronize. Prefer listener setup and thread joins.

### Step 2: replace `read_line(String)` with a fixed-size status-line read

Keep the existing `TcpStream::connect_timeout` plus read/write socket timeouts.

Use a small stack buffer. A maximum first-line size in the range of 256-1024 bytes is more than sufficient for Gregg's health endpoint; choose one documented constant and keep it local, for example:

```rust
const MAX_STATUS_LINE_BYTES: usize = 512;
```

Read incrementally until one of these conditions occurs:

- `\r\n` is found -> parse only bytes before CRLF;
- EOF occurs before CRLF -> return a premature-EOF/malformed-response error;
- the fixed buffer fills without CRLF -> return a status-line-too-long error;
- socket read times out/fails -> return the existing bounded probe diagnostic.

Do not allocate an unbounded `String`. Parsing can operate on the bounded byte slice and convert only that slice to ASCII/UTF-8 if convenient.

A suitable implementation shape is:

```text
read_status_line(&mut TcpStream) -> Result<&bounded bytes / small owned value, CroncheckError>
parse_health_status(line)        -> Result<(), CroncheckError>
```

The exact helper split is optional. Avoid introducing an enum hierarchy unless it actually makes the tests and error reporting smaller.

### Step 3: make status-line validation deliberately narrow

After obtaining a complete line:

- require a valid text representation for the status line;
- split on ASCII whitespace;
- require version exactly `HTTP/1.0` or `HTTP/1.1`;
- require a three-digit numeric status code token;
- accept only `200`;
- reject missing version/code, malformed code, unsupported version, or any non-200 status.

The reason phrase does not need interpretation. Do not parse headers or body.

Do not loosen the parser to support hypothetical protocols. The probe is deliberately matched to Gregg's existing HTTP/1 server.

### Step 4: remove the permanently disabled legacy test module

Delete the `#[cfg(all(test, any()))] mod tests { ... }` block completely.

Before deleting, inventory its still-relevant assertions against current active coverage.

Retain or adapt only tests that protect current behavior and are not already covered elsewhere. Likely candidates to verify before deletion include:

- explicit missing config path fails without writing;
- implicit/default-path config loading starts from defaults;
- validation failure does not overwrite valid config;
- exit-code classification for configuration and permission errors;
- path-with-spaces atomic-write behavior if no equivalent `config.rs` test exists.

Rules:

1. Prefer an existing equivalent test over copying another test into `cli.rs`.
2. If current behavior is already covered in `config.rs` or `main.rs`, delete the legacy duplicate.
3. If coverage is genuinely missing, port one focused test to the active module using `mutate_config` / current dispatch signatures.
4. Do not retain fake Unix service managers, `systemctl` strings, `mutate_and_restart`, or old `croncheck`-starts-service semantics.
5. Do not replace the dead module with another large compatibility test module.

A source search after cleanup should find no `cfg(all(test, any()))`, no stale `mutate_and_restart` test references, and no Unix service-manager behavior in active or dead daemon tests.

### Step 5: run focused local verification

Run at minimum:

```bash
cargo fmt --all -- --check
cargo test -p greggd cli
cargo test -p greggd --bin greggd
cargo test -p greggd
./scripts/check-local.sh
```

Also run direct source checks:

```bash
rg 'cfg\(all\(test, any\(\)\)\)' crates/greggd/src
rg 'mutate_and_restart|systemctl start greggd|systemctl restart greggd' crates/greggd/src
rg 'Command::new\("systemctl"\)|Command::new\("launchctl"\)' crates/greggd/src
```

Expected result: no matches for dead-test gating or Unix service-manager runtime/test behavior. Windows-specific SCM code remains expected.

Do not add these commands to CI.

### Step 6: rerun one narrow local Ubuntu operational smoke

Because the change touches the live `croncheck` parser, repeat only the operational portion necessary to prove no regression. Do not repeat unrelated release qualification.

On the current Ubuntu host:

1. build `greggd` release binary;
2. create a temporary user-owned config on an unused loopback port;
3. start `target/release/greggd run --config <temp-config>` directly as the current user;
4. confirm `/v2/healthz` returns HTTP 200;
5. run `target/release/greggd --config <temp-config> croncheck` and require exit `0`;
6. terminate the daemon normally;
7. run the same `croncheck` and require prompt nonzero exit;
8. confirm the stopped check did not start a replacement daemon and the endpoint remains unreachable;
9. clean up process/config even on failure.

The smoke must use no `sudo`, `systemctl`, `service`, `loginctl`, or `pkexec`.

This remains local-only. No GitHub Actions modification is permitted for Plan 077.

### Step 7: reconcile planning records directly

After all implementation and local verification succeeds:

1. Set Plan 076 status to `complete`.
2. Mark Plan 076 acceptance criteria complete only where the final source and repeated local smoke demonstrate them.
3. Preserve its original implementation SHA and verification record, adding a short note that Plan 077 corrected strict status-line bounds/EOF handling and removed stale disabled tests.
4. Mark Plan 077 complete with its implementation SHA and concise verification result.
5. Update `plans/README.md` to show Plans 076 and 077 complete and no active corrective work.
6. Extend the dependency chain to `... -> 075 -> 076 -> 077` where that chain is maintained.
7. Remove or update stale text saying not to create Plan 076 now that 076 legitimately exists for product work.
8. Do not create Plan 078 or a separate evidence document solely for closure.

## Acceptance criteria

### Bounded and strict `croncheck`

- [ ] `probe_health()` no longer uses an unbounded `String`/`read_line()` for peer-controlled status-line bytes.
- [ ] A fixed maximum status-line byte length is enforced.
- [ ] A complete `\r\n` terminator is required before the status line is accepted.
- [ ] EOF before CRLF returns nonzero.
- [ ] A status line exceeding the fixed maximum returns nonzero without input-proportional allocation.
- [ ] Only exact supported HTTP/1.0 or HTTP/1.1 version tokens are accepted.
- [ ] Status `200` succeeds; 503 and every other status fail.
- [ ] Read/write/connect failures and timeouts remain bounded and diagnostic.
- [ ] No response body parsing, retry loop, new HTTP dependency, or generalized parser is introduced.

### Regression coverage

- [ ] Active tests cover HTTP 200 success.
- [ ] Active tests cover HTTP 503 failure.
- [ ] Active tests cover plainly malformed status text.
- [ ] Active tests cover premature EOF before CRLF.
- [ ] Active tests cover an overlong first status line.
- [ ] Active tests cover invalid HTTP version text.
- [ ] Active tests cover refused/closed-port failure within the fixed timeout.
- [ ] Wildcard IPv4/IPv6 probe normalization tests remain present.
- [ ] Config-only Unix mutation/version tests remain present or have equivalent current coverage.

### Test-source cleanup

- [ ] The always-false `#[cfg(all(test, any()))]` legacy module is deleted.
- [ ] No stale fake Unix service-manager tests remain.
- [ ] No stale `croncheck`-starts-service assertions remain.
- [ ] No stale `mutate_and_restart` test path remains.
- [ ] Still-useful non-duplicated config/error regression coverage is retained in the smallest appropriate existing module.
- [ ] The cleanup does not expand into a broad test refactor.

### Local verification

- [ ] `cargo fmt --all -- --check` passes.
- [ ] Focused `greggd` CLI tests pass.
- [ ] `greggd` binary/package tests pass.
- [ ] `./scripts/check-local.sh` passes.
- [ ] Source searches show no Unix `systemctl`/`launchctl` production invocation and no dead always-false test module.
- [ ] Release `greggd` builds on the current Ubuntu host.
- [ ] Live local daemon health returns 200 and live `croncheck` exits 0.
- [ ] After normal daemon termination, `croncheck` exits nonzero promptly and does not restart the daemon.
- [ ] Local smoke uses no privilege escalation or service manager.
- [ ] No CI workflow/job/step is added or changed for this phase.

### Scope and planning closure

- [ ] Plan 076 is reconciled to `complete` with truthful checked acceptance criteria after the correction.
- [ ] Plan 077 is marked complete with implementation SHA and concise local verification record.
- [ ] `plans/README.md` lists 076 and 077 accurately and shows no active corrective work.
- [ ] Dependency ordering includes 076 -> 077.
- [ ] No Plan 078 or separate evidence file is created solely for closure.
- [ ] No unrelated protocol, collector, TUI, EggPool, drive, release, packaging, SCM, or CI work is included.

## Handoff

Implementation handoff should report only the material result:

```text
Implementation SHA: <sha>
croncheck bounds: <fixed maximum + CRLF/EOF behavior>
HTTP validation: <accepted versions/status semantics>
Regression tests: <200/503/malformed/EOF/overlong/version/refusal>
Legacy test cleanup: <always-false module removed; current coverage retained>
Local verification: <fmt/tests/check-local + Ubuntu live/dead croncheck>
CI changes: none
Planning closure: Plans 076-077 complete; index reconciled
Remaining work: none / concrete defect only
```
