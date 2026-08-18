# Phase 083: compact TUI, endpoint nicknames, and polling invariant

Status: planned.

Depends on: completed Plan 082 and current client behavior on `main` as reviewed at `561e859024edb8aaa670f0e710dd81c55c6f7b04`.

## Objective

Make the `gregg` client materially easier to read on small terminals without expanding its scope or changing the daemon/protocol architecture.

This phase is limited to six related client corrections:

1. make the normal-view CPU/MEM/SWP-or-COMMIT/DISK block visually compact and column-aligned;
2. remove redundant `used` / `avail` words from aggregate disk usage text;
3. guarantee a fresh `gregg` launch begins at the top of the post-poll display order instead of following a configured system that moved downward after reachability sorting;
4. require an explicit host/port pair when adding a monitored system and add the ergonomic `nickname@host:port` form;
5. persist and render the nickname through the existing `SystemEntry.name` field without changing the configuration schema;
6. preserve and explicitly test the existing invariant that offline systems continue to be polled on later refresh generations so they can recover automatically.

This is a bounded client UI/CLI correctness phase. It is not a TUI redesign, scheduler rewrite, daemon change, protocol change, configuration migration, or observability expansion.

## Scope decisions

### `gregg add` now requires an explicit port

Interpret the requested host/port validation literally for the add path.

After this phase, these forms are valid:

```text
gregg add 192.168.182.146:11310
gregg add server.local:11310
gregg add [fd00::10]:11310
gregg add http://server.local:11310/
gregg add deadpool@192.168.182.146:11310
gregg add deadpool@server.local:11310
```

These forms are invalid because the port is implicit or missing:

```text
gregg add 192.168.182.146
gregg add server.local
gregg add ::1
gregg add http://server.local/
gregg add deadpool@192.168.182.146
```

Do **not** make the canonical endpoint parser globally require a port. Host-only parsing remains useful for existing `gregg remove HOST` semantics, where host-only input intentionally removes all configured ports for that host.

The narrow correction is therefore an add-command requirement checked after ordinary endpoint parsing. Existing `EndpointSpec.port_was_explicit` is already the authoritative signal.

### Inline nickname syntax is an add-command convenience

`nickname@host:port` is a `gregg add` convention, not a new network endpoint grammar.

Do not weaken `EndpointSpec::parse()` so arbitrary `@` is accepted. The canonical endpoint parser must continue rejecting credentials/userinfo.

The add path should extract an optional nickname before handing the remaining address to the existing endpoint parser. Persist the nickname as `SystemEntry.name` exactly as `--name` already does.

Retain `--name` for compatibility. If both inline nickname syntax and `--name` are supplied, reject the command as ambiguous instead of silently choosing one.

HTTP URL credential rejection must remain intact. An input beginning with an HTTP URL, such as:

```text
http://user:password@host:11310/
```

must continue to be parsed as a URL and rejected for credentials; the `user:password` portion must not be reinterpreted as a Gregg nickname.

A small CLI-local helper such as `parse_add_target()` is preferable to changing the general endpoint grammar.

### Configuration schema does not change

`SystemEntry` already has:

```rust
pub name: Option<String>
```

and current configuration already serializes it as:

```toml
[[systems]]
id = "..."
host = "192.168.182.146"
port = 11310
name = "deadpool"
```

Reuse this field. Do not add `nickname`, alias tables, daemon-side naming, discovery, migrations, or a second identity field.

### Offline polling remains fixed-cadence

The current scheduler already polls the full configured endpoint list on every generation. Reachability state is not used to remove or suppress offline endpoints.

Preserve that model. Do not add a per-device retry scheduler, exponential-backoff state machine, retry queue, or new dependency merely because backoff would be theoretically possible.

A future bounded backoff may be considered separately if measured fleet behavior justifies it. This phase only needs a regression test proving that a failed endpoint is polled again and can recover.

## Baseline findings

### 1. Normal-view bars are laid out independently

`crates/gregg/src/ui/bar.rs::render_bar()` currently derives each row's fixed width from that row's own label length, percentage string, and optional detail string.

Consequences:

- `CPU`, `MEM`, and `SWP` have three-character labels while `DISK` has four characters, so their `[` positions differ;
- Windows may render `COMMIT`, which is six characters and makes the mismatch larger;
- detail lengths differ between CPU, memory, swap/commit, and disk, so the computed bar width differs by row and the closing `]` columns do not align;
- unavailable rendering has a separate width calculation and can diverge from available rows;
- the current detail truncation budget is independent per row rather than being derived from the whole metric block.

The requested display requires one group-level geometry calculation for the complete metric block.

### 2. Aggregate disk detail is unnecessarily verbose

`crates/gregg/src/ui/system_block.rs::render_disk_and_drives()` currently constructs:

```text
283.8 GiB used / 167.1 GiB avail
```

The desired text is:

```text
283.8 GiB / 167.1 GiB
```

No metric meaning changes. The first value remains used bytes and the second remains caller-available bytes from the existing aggregate.

### 3. Startup selection can pull the viewport away from the top

`AppState::from_config()` initially selects the first configured system and sets `viewport_top_id` to the same system.

After the first poll, `display_order()` moves online systems before offline/pending systems. `apply_batch()` then calls `ensure_selected_visible()`.

If the first configured system is offline while a later system is online, the initially selected system moves down the display order and the viewport follows it. A fresh TUI can therefore appear already scrolled away from the top.

The first accepted poll batch should establish the initial post-reachability display order and place both selection and viewport at that order's first entry. Later generations must preserve ordinary user selection/scroll behavior.

### 4. Name persistence and most name rendering already exist

`SystemEntry.name`, `SystemState.configured_name`, the normal header, and the condensed view already support a configured client-side display name.

Do not replace this plumbing.

The main missing work is add-command nickname parsing plus one offline-row cleanup. The normal offline renderer currently uses the endpoint host as a fallback display name and then also prints the endpoint address, which can produce a redundant unnamed form resembling:

```text
192.168.182.146@192.168.182.146:11310 offline
```

The intended forms are:

```text
deadpool@192.168.182.146:11310 offline
192.168.182.146:11310 offline
```

The `name@` prefix exists only when a name is configured.

### 5. The add path currently permits implicit default ports

`cmd_add()` calls `EndpointSpec::parse_add_input()` and resolves `config.default_port` whenever `port_was_explicit == false`.

That is the exact behavior to tighten. Do not delete `default_port` from the configuration schema in this phase; it may still be part of historical configuration and other parsing behavior. The product-facing `gregg add` command simply stops accepting an omitted port.

### 6. Offline endpoints are already retained by the scheduler

`PollScheduler::poll_generation()` iterates the complete `endpoints` slice each generation and creates one result per configured endpoint.

An offline result only changes `SystemState.reachability`; it does not mutate the scheduler endpoint list.

No production scheduler correction is currently required. Add regression coverage and change production logic only if implementation-time inspection discovers behavior that contradicts this baseline.

## Authoritative normal-view layout

The exact number of bar cells depends on terminal width, but the geometry must follow this model:

```text
deadpool  IO 0.4%  1.32/0.91/0.62  8c

    CPU  [||||||||||||||||              ] 25.2% 8 cores
    MEM  [||||||||||||||||||||          ] 37.8% 5.9 GiB/15.6 GiB
    SWP  [                              ] 0.0%
    DISK [||||||||||||||||||||||||      ] 60.4% 283.8 GiB / 167.1 GiB
```

Requirements:

- every metric row begins with exactly four spaces;
- all opening `[` characters in one system block occupy the same terminal column;
- all closing `]` characters in one system block occupy the same terminal column;
- labels are padded to the widest label actually present in the block;
- Windows `COMMIT` participates in the same calculation, so hard-coding a four-character label column is incorrect;
- the suffix after `]` contains percentage plus optional detail;
- aggregate disk detail contains no `used` or `avail` words;
- a row must never exceed the available terminal width;
- terminal display width, not UTF-8 byte length, is authoritative where width calculations are involved.

A suitable line model is:

```text
"    {label:<label_width} [{bar}] {suffix}"
```

where `label_width` and `bar` width are derived once for all metric rows.

## Group-level bar-width algorithm

Do not render the four metric rows as four unrelated width calculations.

For each system block:

1. Build row descriptions for CPU, MEM, SWP-or-COMMIT, and DISK before drawing any bar.
2. Determine the maximum terminal-cell width of the labels actually present.
3. Build each row's desired suffix text after the closing bracket.
   - available metric: percentage is mandatory;
   - optional detail follows the percentage when space allows;
   - unavailable metric: use the existing unavailable marker semantics rather than a fabricated `0.0%`.
4. Measure each final suffix with `unicode_width` and determine the maximum suffix width.
5. Reserve the common fixed prefix, both brackets, separator spaces, and that maximum suffix width.
6. Use the remaining columns as **one common bar width** for every row.
7. For each percentage, fill only its fraction of that common width and pad the rest of the common bar width with spaces.
8. Render every row using the same label width and same total bar width.

This directly guarantees aligned `[` and `]` columns.

### Small-width degradation

The phase is specifically intended to improve small screens, so long details must not consume the entire bar.

At supported widths where the full desired details do not leave a useful bar:

1. preserve the four-space indent, label, brackets, and percentage;
2. truncate or drop optional detail before sacrificing the percentage;
3. recompute the maximum suffix width from the compacted suffixes;
4. retain at least one bar cell whenever the supported terminal width makes that possible;
5. never append an ellipsis outside the width budget.

Do not add a large tier system or a second layout engine. A small deterministic width helper is sufficient.

If the existing `truncate_str()` helper remains, correct its ellipsis accounting so a string declared to fit `N` display columns cannot become `N + 1` after appending `…`.

## Implementation sequence

### Step 1: introduce one shared normal-metric layout path

Primary files:

```text
crates/gregg/src/ui/bar.rs
crates/gregg/src/ui/system_block.rs
```

Refactor only enough for the four-row normal metric block to share geometry.

Acceptable shapes include:

- a small row-spec struct plus `render_metric_group()`;
- a small group-layout helper calculated in `system_block.rs` and passed to row rendering;
- another equivalently small design that calculates label and suffix widths once.

Avoid a generic layout framework, widget hierarchy, trait abstraction, or new crate.

The helper must support:

- CPU;
- MEM;
- SWP;
- Windows COMMIT;
- DISK;
- unavailable DISK;
- zero-size swap detail behavior already represented by the current normalized model.

### Step 2: make suffix text concise and width-aware

Primary files:

```text
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/text.rs
crates/gregg/src/ui/bar.rs
```

Required text changes:

```text
DISK ... 60.4% 283.8 GiB / 167.1 GiB
```

not:

```text
DISK ... 60.4% 283.8 GiB used / 167.1 GiB avail
```

Keep existing byte units and percentage formatting unless a width bug requires a narrowly scoped correction.

Memory/swap/commit details may retain their current `used/total` compact value forms.

Use `unicode_width` for terminal-cell accounting. Do not add another width dependency.

### Step 3: guarantee fresh launch starts at display-order position zero

Primary file:

```text
crates/gregg/src/state.rs
```

Use the first accepted poll generation as the boundary that establishes initial reachability ordering.

Preferred behavior:

```text
before first accepted batch:
    configured order may be pending

after first accepted batch:
    order = display_order()
    selected_id = first(order)
    viewport_top_id = selected_id

later batches:
    preserve current selection/viewport semantics
```

The implementation may use `last_applied_generation == 0` before applying the first accepted batch or an equivalently small existing signal. Do not add a new scroll state machine.

Important boundaries:

- this reset is launch-initialization behavior only;
- do not snap to the top after every periodic poll;
- do not snap to the top on ordinary `Ctrl-R` reload if the current stable selection remains valid;
- retain current `ensure_selected_visible()` behavior after initialization;
- empty-system behavior remains unchanged.

### Step 4: add inline nickname parsing and explicit-port enforcement

Primary files:

```text
crates/gregg/src/cli.rs
crates/gregg/src/endpoint.rs   # only if a small error variant/helper belongs here
```

Add a narrow add-command parse layer that produces:

```text
optional configured name
EndpointSpec
```

For ordinary non-URL input containing `@`, split the nickname from the endpoint once, validate the nickname with the existing `validate_name()`, then parse the remainder with `EndpointSpec::parse_add_input()`.

Preserve HTTP URL credential semantics. A practical rule is:

- if the full input starts as an HTTP URL, parse it as an HTTP URL and do not treat its userinfo `@` as nickname syntax;
- otherwise an initial `nickname@...` prefix may be extracted and the remainder may itself use an accepted add endpoint form if the implementation can do so without weakening credential checks.

After endpoint parsing, require `spec.port_was_explicit == true` for `cmd_add()`.

Introduce a clear endpoint/add error such as `explicit port required` and keep its exit classification as the existing endpoint error exit code.

Do not change host-only `remove` matching.

Do not change HTTPS rejection.

Do not change canonical host normalization or IPv6 bracket rules.

### Step 5: persist and render configured names without schema changes

Primary files:

```text
crates/gregg/src/cli.rs
crates/gregg/src/ui/system_block.rs
```

When an inline nickname is supplied:

```text
gregg add deadpool@192.168.182.146:11310
```

persist:

```toml
name = "deadpool"
host = "192.168.182.146"
port = 11310
```

The existing configured-name path should then drive normal and condensed display.

Retain `--name` as an alternate explicit form:

```text
gregg add 192.168.182.146:11310 --name deadpool
```

Reject:

```text
gregg add deadpool@192.168.182.146:11310 --name other-name
```

because two distinct name sources were supplied.

Normal online header behavior:

- configured `name`: header begins with that name;
- no configured `name`: retain the existing host-based header behavior.

Condensed view behavior:

- configured `name`: host/name column uses that name as today;
- no configured `name`: retain existing endpoint-host behavior.

Normal offline behavior:

```text
named:   deadpool@192.168.182.146:11310 offline
unnamed: 192.168.182.146:11310 offline
```

Do not derive the client nickname from daemon-reported `system.name`; the operator-configured client `name` remains authoritative for this feature.

### Step 6: lock in continued polling of offline systems

Primary file:

```text
crates/gregg/src/scheduler.rs
```

First inspect current behavior again. If it remains as reviewed, make no scheduler production change.

Add a deterministic test that demonstrates recovery behavior across generations. Preferred test shape:

1. configure one endpoint backed by a local mock;
2. first poll attempt returns a failure outcome that marks the system unavailable/offline;
3. allow the next scheduler generation to run using a short test interval or injected test behavior, not a production-duration sleep;
4. second attempt returns a valid Gregg payload;
5. assert the same endpoint was polled again and the second batch reports it online.

A two-generation failure/failure assertion is acceptable if the existing mock structure makes failure-then-success disproportionately invasive, but recovery proof is stronger and preferred.

Preserve:

- one result per configured endpoint per generation;
- semaphore concurrency bound;
- fixed cadence;
- no overlapping generation architecture changes;
- cancellation semantics;
- panic-to-`Cancelled` conversion.

Do not add offline backoff in this phase.

### Step 7: update user-facing documentation after behavior is implemented

Expected documentation surface:

```text
README.md
AGENTS.md
architecture/gregg-client.md          # only where current add/UI semantics are described
crates/gregg/config.example.toml      # only if an example clarification is useful
plans/README.md
plans/083-compact-tui-endpoint-nicknames-and-polling-invariant.md
```

Required documentation corrections:

- examples of `gregg add` use explicit ports;
- document `nickname@host:port`;
- keep `--name` documented if it remains supported;
- show compact aggregate disk text without `used` / `avail`;
- show four-space metric indentation and aligned normal-view bars;
- state that offline systems continue periodic polling and recover automatically when reachable again if this behavior is currently documented elsewhere;
- update `AGENTS.md` so future agents do not reintroduce implicit-port `gregg add` examples;
- add Plan 083 to `plans/README.md` as the active phase while implementation is in progress.

Do not rewrite unrelated historical plans merely because their examples used the old display format. Historical plans remain historical unless they are currently normative documentation.

### Step 8: close this phase in place

When all acceptance criteria pass:

1. mark Plan 083 complete;
2. record the implementation SHA;
3. record the focused commands actually run;
4. update `plans/README.md` to mark the phase complete;
5. if ordinary existing CI ran, record the run once, but do not make repeated CI runs a standing requirement;
6. do not create Plan 084 solely to record closure.

Create a follow-up phase only if review finds a concrete unresolved product defect.

## Focused test requirements

### Normal-view geometry

Use the existing ratatui `TestBackend`-based rendering tests rather than adding screenshot infrastructure.

Cover representative widths such as:

```text
24
32
40
60
80
```

or the nearest set that exercises minimum, narrow, medium, and ordinary widths.

For one online Linux-shaped system, assert:

- CPU/MEM/SWP/DISK metric rows each begin with exactly four spaces;
- the terminal-column index of `[` is identical on all four metric rows;
- the terminal-column index of `]` is identical on all four metric rows;
- the disk row contains the two byte values separated by `/`;
- the disk row does not contain `used`;
- the disk row does not contain `avail`;
- no rendered metric row exceeds the test backend width.

For a Windows-shaped system, assert:

- CPU/MEM/COMMIT/DISK share the same `[` and `]` columns;
- the longer `COMMIT` label does not move its opening bracket relative to the other rows.

For unavailable drive data, assert:

- DISK uses the same shared bracket geometry;
- unavailable state remains `—` or the existing truthful unavailable representation;
- unavailable DISK does not fabricate `0.0%`.

Add a narrow-width case with long detail values that proves detail compaction does not shift the brackets or overflow the terminal.

### Initial viewport behavior

Add a state test with at least three configured systems where:

- configured system 0 becomes offline;
- configured system 1 or 2 becomes online in the first accepted batch;
- online-first `display_order()` therefore differs from configured order.

After the first batch, assert:

```text
selected_id == display_order()[0]
viewport_top_id == display_order()[0]
```

Then apply a later generation and assert ordinary selection is not reset to the top merely because reachability changed.

If practical, add a `Ctrl-R` reconciliation regression proving a valid stable selection remains preserved.

### Add-command endpoint validation

Add focused CLI/endpoint tests for:

Accepted:

```text
host:11310
192.168.182.146:11310
[fd00::10]:11310
http://host:11310/
deadpool@host:11310
deadpool@192.168.182.146:11310
```

Rejected:

```text
host
192.168.182.146
::1
http://host/
deadpool@host
deadpool@
@host:11310
host:0
host:notaport
host:99999
```

Retain tests proving URL credentials are rejected.

Add a test proving host-only `gregg remove HOST` semantics still work.

### Nickname persistence and display

Using a temporary config store, assert:

```text
gregg add deadpool@192.168.182.146:11310
```

persists one system with:

```text
name == Some("deadpool")
host == "192.168.182.146"
port == 11310
```

Also assert:

- ordinary `gregg add host:port` persists `name == None`;
- `nickname@host:port` plus `--name` is rejected;
- invalid/empty/overlong nickname uses the existing name validation path;
- named offline rendering uses exactly one `name@` prefix;
- unnamed offline rendering does not duplicate the host before `@`;
- existing normal/condensed configured-name behavior remains intact.

### Offline retry/recovery

Add a scheduler regression demonstrating that a failed endpoint remains in later generations and is polled again.

Prefer failure-then-success so the test proves automatic recovery without a manual refresh.

Do not use sleeps on the production refresh interval.

## Verification

Run the smallest useful focused set first. Use the actual test filter names that exist after implementation.

Expected commands:

```bash
cargo fmt --all -- --check
cargo test -p gregg ui
cargo test -p gregg state
cargo test -p gregg cli
cargo test -p gregg endpoint
cargo test -p gregg scheduler
cargo test -p gregg --bin gregg
./scripts/check-local.sh
```

If a filter matches no tests, replace it with the nearest exact module/test invocation and record the command actually used.

A release preflight is not required solely for this client display/CLI phase unless implementation unexpectedly changes packaging or release-facing files beyond documentation.

No new CI workflow, job, matrix, artifact upload, screenshot gate, terminal emulator harness, or evidence bundle is required.

The existing cross-platform CI should remain green if it runs naturally because `gregg` is cross-platform, but this phase does not add another verification layer.

### Optional local visual smoke

After deterministic renderer tests are green, a brief interactive TUI check on a real terminal at roughly 40-60 columns is useful but is not a substitute for the automated geometry assertions.

Inspect one named and one unnamed system if available and confirm visually:

```text
four-space metric indent
aligned [ columns
aligned ] columns
compact disk suffix
startup positioned at the top
```

Do not build a screenshot automation system for this.

## Expected implementation surface

Primary production files:

```text
crates/gregg/src/ui/bar.rs
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/text.rs            # only width/truncation helpers as needed
crates/gregg/src/state.rs
crates/gregg/src/cli.rs
crates/gregg/src/endpoint.rs           # only add error/helper if warranted
```

Regression-test-only or test-adjacent surface:

```text
crates/gregg/src/ui/mod.rs
crates/gregg/src/scheduler.rs
```

Documentation after implementation:

```text
README.md
AGENTS.md
architecture/gregg-client.md           # only relevant current semantics
plans/README.md
this plan
```

Files that should not require product changes:

```text
crates/greggd/**
crates/gregg-protocol/**
.github/workflows/**
scripts/**
packaging/**
```

If implementation begins modifying those areas, stop and confirm that a concrete requirement from this plan actually requires it.

## Explicit non-goals

Do not add any of the following:

- daemon-side nickname synchronization;
- hostname discovery or mDNS discovery;
- aliases independent of `SystemEntry.name`;
- config schema version 2;
- config migration machinery;
- implicit port inference for the new add behavior;
- TLS/HTTPS support;
- authentication;
- per-device retry queues;
- exponential backoff state machines;
- new async actors/channels for reachability;
- TUI colors/themes/icons;
- a new condensed-view design;
- process monitoring;
- historical telemetry;
- alerting;
- new dependencies;
- new CI workflows or evidence infrastructure;
- unrelated cleanup while touching client files.

## Acceptance criteria

### Visual compactness and alignment

- [ ] CPU, MEM, SWP-or-COMMIT, and DISK rows begin with exactly four spaces in normal view.
- [ ] All normal metric rows in the same system block render `[` at the same terminal column.
- [ ] All normal metric rows in the same system block render `]` at the same terminal column.
- [ ] Label alignment is derived from the widest actual label and handles Windows `COMMIT` correctly.
- [ ] Closing-bracket alignment is derived from the longest final usage suffix rather than independent row widths.
- [ ] Aggregate disk detail is rendered as `<used bytes> / <available bytes>` without `used` or `avail` words.
- [ ] Available metric percentages remain visible before optional details are dropped on narrow screens.
- [ ] Unavailable metrics remain truthful and use the shared geometry.
- [ ] No metric row exceeds the terminal width at the supported minimum width or representative narrow widths.
- [ ] Any retained ellipsis-based truncation respects its declared display-column budget.

### Startup viewport

- [ ] After the first accepted poll batch, a fresh session selects display-order position zero.
- [ ] After the first accepted poll batch, `viewport_top_id` is display-order position zero.
- [ ] An initially configured offline system cannot cause the fresh TUI to start scrolled below online systems.
- [ ] Later periodic batches do not continually snap the user back to the top.
- [ ] Ordinary valid `Ctrl-R` reconciliation keeps existing stable selection behavior rather than acting like a new process launch.

### Endpoint add validation

- [ ] `gregg add` requires an explicit nonzero port.
- [ ] Host-only IPv4/DNS input is rejected by `gregg add`.
- [ ] Bare IPv6 without an explicit bracketed port is rejected by `gregg add`.
- [ ] HTTP URL input without an explicit port is rejected by `gregg add`.
- [ ] Explicit IPv4, DNS, bracketed IPv6, and HTTP URL host/port inputs continue to work.
- [ ] Host-only `gregg remove HOST` semantics remain unchanged.
- [ ] Existing credential, path, unsupported-scheme, invalid-port, zero-port, and overflow rejection remains intact.

### Nicknames

- [ ] `gregg add nickname@host:port` is accepted for a valid nickname and endpoint.
- [ ] Inline nickname is persisted through the existing `SystemEntry.name` field.
- [ ] No configuration schema/version change is introduced.
- [ ] Existing `--name` remains supported.
- [ ] Supplying both inline nickname and `--name` is rejected as ambiguous.
- [ ] Existing name validation applies to inline nicknames.
- [ ] A configured nickname appears at the start of the normal device header.
- [ ] A configured nickname remains used by condensed view.
- [ ] Named offline rows render `name@host:port offline`.
- [ ] Unnamed offline rows render `host:port offline` without a synthetic name prefix or duplicate host.
- [ ] HTTP URL credentials are not reinterpreted as nickname syntax.

### Offline polling

- [ ] An endpoint that fails one generation remains configured in the scheduler.
- [ ] The scheduler polls that endpoint again on a later periodic generation without manual re-add.
- [ ] Regression coverage demonstrates later recovery when the endpoint becomes healthy, if practical with the existing mock structure.
- [ ] No new backoff/retry state machine is added.
- [ ] Existing scheduler concurrency, generation, cancellation, and one-result-per-endpoint invariants remain intact.

### Verification and scope

- [ ] Focused UI, state, CLI/endpoint, and scheduler tests pass.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `./scripts/check-local.sh` passes.
- [ ] Documentation examples reflect explicit-port add syntax, nickname syntax, and compact disk rendering.
- [ ] No new dependency is added.
- [ ] No `greggd` or `gregg-protocol` product behavior is changed.
- [ ] No new CI workflow/job/matrix/evidence mechanism is added.
- [ ] `plans/README.md` and this plan are reconciled with the final implementation state.

## Completion rule

Plan 083 is complete only when every applicable acceptance criterion above is implemented and demonstrated by focused deterministic tests plus the existing local check.

Do not mark it complete because the code compiles or because the display looks plausible in one terminal.

The critical closure proofs are:

```text
shared [ and ] columns at narrow and ordinary widths
compact disk text
first-generation viewport at display-order zero
explicit-port add rejection/acceptance matrix
nickname persistence and named/unnamed rendering
failed endpoint polled again on a later generation
./scripts/check-local.sh green
```

If those pass and no concrete defect remains, close this phase directly. Do not create a closure-only follow-up plan.