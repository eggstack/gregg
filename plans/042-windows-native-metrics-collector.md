# Phase 42: Windows native metrics collector

## Objective

Implement a native Windows collector for `greggd` that produces truthful version-2 snapshots using Windows system APIs.

The collector must provide:

- stable system identity;
- logical processor count;
- aggregate CPU utilization derived from counter deltas;
- physical memory used/total;
- Windows commit used/limit;
- explicit absence of Unix load average, Unix swap, and CPU I/O-wait.

The collector must fit the existing `SystemCollector` boundary, remain independent of HTTP requests, warm counter-based metrics before readiness, and expose deterministic source seams for unit tests.

## Dependency and execution position

Depends on Phase 41 freezing the v2 capability and snapshot model.

Builds on the Windows client/platform foundation from Phase 40.

Must complete before:

- Phase 43 integrates the collector into a Windows service runtime;
- Phase 44 adds final native Windows daemon CI and documentation closure.

## Governing invariants

1. The collector uses native Windows APIs and never shells out to PowerShell, WMIC, `systeminfo`, `typeperf`, or other commands.
2. CPU utilization is derived from two valid counter samples.
3. Counter reset, regression, zero delta, and overflow conditions are handled explicitly.
4. Windows load average is unsupported, not synthesized.
5. Windows swap is unsupported unless a future design identifies a semantically equivalent native measure.
6. Windows commit usage is reported under the dedicated commit metric.
7. CPU I/O-wait is unsupported.
8. Native API failure produces a typed collector error and truthful readiness/failure state.
9. Unsafe code is isolated behind a narrow Windows source module.
10. HTTP requests read cached snapshots and never trigger native collection.
11. The initial supported Windows host limit is explicit; systems outside it fail clearly rather than producing partial aggregate CPU data.
12. No release-evidence workflow is added.

## Scope

### In scope

- `cfg(target_os = "windows")` collector module;
- target-specific Windows API dependency;
- identity source and normalization;
- CPU counter source and delta math;
- logical processor count;
- physical memory source;
- commit source;
- capability mapping;
- Windows collector errors;
- source abstraction/test doubles;
- native unit/integration/runtime smoke tests;
- daemon binary collector selection on Windows;
- v2 snapshot production;
- documentation of limitations.

### Out of scope

- Windows service control/runtime;
- installer creation;
- per-core or per-process metrics;
- GPU, disk, network, temperature, battery, or process monitoring;
- Unix load emulation;
- pagefile file-by-file inspection;
- public-internet hardening;
- Windows ARM64 support claims;
- hosts with more logical processors than the chosen correct aggregate implementation can cover, unless implemented and tested in this phase;
- automated publication.

## Workstream A: establish the Windows collector module boundary

Add a module structure comparable to Linux/macOS:

```text
crates/greggd/src/collector/windows/
  mod.rs
  source.rs
  identity.rs
  tests.rs
```

or another small equivalent structure.

Expose:

```rust
pub struct WindowsCollector<S = NativeWindowsSource> { ... }
```

where a source trait or set of narrow traits supplies raw API data for deterministic tests.

Recommended source responsibilities:

```rust
trait WindowsSource {
    fn cpu_times(&self) -> Result<CpuTimes, CollectError>;
    fn active_processor_count(&self) -> Result<u32, CollectError>;
    fn physical_memory(&self) -> Result<PhysicalMemoryRaw, CollectError>;
    fn commit_memory(&self) -> Result<CommitRaw, CollectError>;
    fn identity(&self) -> Result<SystemIdentityRaw, CollectError>;
}
```

A split into smaller traits is acceptable if it improves testing without introducing abstraction noise.

### Unsafe-code policy

The workspace denies unsafe code by default. Permit unsafe code only in the contained native Windows source module using the narrowest possible attribute. Safe collector logic, delta math, normalization, and tests must remain safe Rust.

Document every unsafe call with:

- buffer initialization/size contract;
- pointer validity;
- return-value/error handling;
- units and conversion assumptions;
- ownership/lifetime behavior.

### Dependency policy

Use a target-specific Windows API crate under:

```toml
[target.'cfg(windows)'.dependencies]
```

Verify compatibility with the workspace MSRV. Enable only required feature modules. Do not add a broad cross-platform system-information crate that duplicates the existing native collectors.

### Workstream A acceptance criteria

- [ ] Windows code is excluded from non-Windows builds.
- [ ] Native API access is contained behind testable safe interfaces.
- [ ] Unsafe code exists only where required for FFI.
- [ ] Target dependency features are minimal.
- [ ] Linux/macOS collectors remain unchanged in behavior.

## Workstream B: collect and normalize Windows identity

Populate the existing identity fields truthfully:

```text
name
hostname
os_name
os_version
kernel_name
kernel_release
architecture
```

Recommended semantics:

- `name`: configured Gregg display name or existing collector naming policy;
- `hostname`: native computer/DNS hostname;
- `os_name`: `windows`;
- `os_version`: user-recognizable product/version/build string where reliably available;
- `kernel_name`: `Windows NT` or another documented stable value;
- `kernel_release`: numeric build/revision string;
- `architecture`: Rust target architecture normalization, such as `x86_64`.

Avoid deprecated compatibility-sensitive version APIs unless the executable manifest and behavior are explicitly controlled. Prefer a reliable build-number source behind the identity seam. If product-edition naming requires registry access, keep it optional: a numeric Windows version/build is acceptable when product-name lookup fails.

### String handling requirements

- convert UTF-16 safely;
- reject or replace invalid interior NUL assumptions before API calls;
- bound returned string sizes;
- trim trailing NULs and whitespace;
- preserve Unicode hostnames where supported;
- never panic on conversion failure;
- do not expose registry paths or internal API errors in public health responses.

### Required tests

- ordinary hostname/version/build;
- Unicode hostname;
- empty hostname rejected;
- oversized value rejected or bounded;
- product-name source unavailable but numeric version available;
- version source failure;
- architecture normalization;
- configured display-name override.

### Workstream B acceptance criteria

- [ ] Identity fields are stable and human-readable.
- [ ] Build/version behavior is not subject to silent compatibility lies.
- [ ] UTF-16 conversion is bounded and tested.
- [ ] Optional product-name enrichment failure does not destroy otherwise valid identity.
- [ ] Public errors remain coarse.

## Workstream C: implement logical processor count and host-size guard

Use the Windows active-processor API capable of counting across processor groups.

The collector must reconcile processor count with the chosen CPU time source.

### Initial correctness policy

If aggregate CPU time collection is only correct for one processor group or at most 64 logical processors:

- detect multiple processor groups or a count above the supported limit;
- return a typed `UnsupportedHostTopology` collector error;
- document the limitation;
- do not report a system-wide CPU percentage from partial counters.

An implementation that correctly aggregates all processor groups may remove this limitation, but must include native or deterministic tests for multi-group behavior.

Logical core count must be greater than zero and fit the protocol field.

### Required tests

- 1 processor;
- ordinary multi-core count;
- zero returned by API;
- API failure;
- count at supported boundary;
- count above supported boundary;
- multiple processor groups if exposed by the source seam;
- conversion overflow.

### Workstream C acceptance criteria

- [ ] Logical processor count is system-active count, not current process affinity count.
- [ ] Unsupported large topology fails clearly.
- [ ] Partial CPU coverage is never presented as full-system utilization.
- [ ] Boundary conditions are tested.

## Workstream D: implement CPU utilization from system-time deltas

Collect raw idle, kernel, and user time counters.

Windows aggregate system-time semantics include idle time within kernel time. Normalize each sample into monotonic integer counters and compute:

```text
kernel_busy = kernel - idle
total = kernel + user
busy = total - idle
usage_pct = 100 * busy_delta / total_delta
```

Use one formula consistently and document the Windows counter semantics. Perform checked arithmetic before conversion to floating point.

### Collector state

Store only the previous valid raw sample needed for the next delta.

First sample:

```text
CollectErrorKind::Warming
```

Second and subsequent valid samples produce CPU utilization.

### Error/reset behavior

Handle:

- any counter decreasing;
- idle greater than kernel where invalid for the API contract;
- total delta zero;
- checked-add/subtract overflow;
- API failure between samples;
- long pause;
- source returning identical sample;
- percentage outside range because of malformed source data.

Recommended recovery:

- invalid/regressed sample resets baseline and returns warming;
- transient API failure reports collector failure according to existing sampler policy without using stale counters as a new baseline;
- next valid sample establishes/reuses baseline according to a documented deterministic rule;
- final percentage is finite and clamped only after invariants prove the source delta is logically valid. Do not hide invalid math by unconditional clamping.

### Required tests

- first sample warms;
- known 0%, 25%, 50%, and 100% deltas;
- idle included in kernel formula;
- identical counters;
- individual counter regression;
- overflow near integer maximum;
- invalid idle/kernel relation;
- failure then recovery;
- reset then two-sample recovery;
- finite percentage guarantee.

### Workstream D acceptance criteria

- [ ] CPU usage requires two valid samples.
- [ ] Delta formula matches Windows counter semantics.
- [ ] Checked arithmetic is used.
- [ ] Invalid/regressed counters do not produce a ready snapshot.
- [ ] Recovery behavior is deterministic and tested.

## Workstream E: implement physical memory collection

Use the native global memory-status API.

Raw inputs:

```text
total_physical_bytes
available_physical_bytes
```

Normalize:

```text
used = total - available
usage_pct = 100 * used / total
```

Requirements:

- structure length initialized correctly before the API call;
- API return value checked;
- total must be greater than zero;
- available must not exceed total;
- checked subtraction;
- finite percentage in range;
- bytes remain `u64` without lossy intermediate conversion.

Do not use the API's pagefile fields as swap. They reflect system commit semantics and are not the swap metric Gregg exposes on Unix.

### Required tests

- ordinary memory use;
- zero usage;
- full usage;
- available greater than total;
- total zero;
- API failure;
- near-`u64::MAX` values;
- percentage precision bounds.

### Workstream E acceptance criteria

- [ ] Physical memory is derived from total/available bytes.
- [ ] Invalid source relationships fail.
- [ ] Pagefile fields are not mapped to swap.
- [ ] Byte math is checked and lossless until percentage conversion.

## Workstream F: implement Windows commit usage

Use the native performance-information API or equivalent authoritative source for:

```text
commit_total_pages
commit_limit_pages
page_size_bytes
```

Normalize with checked multiplication:

```text
used_bytes = commit_total_pages * page_size_bytes
limit_bytes = commit_limit_pages * page_size_bytes
usage_pct = 100 * used_bytes / limit_bytes
```

Requirements:

- commit total must not exceed limit;
- page size must be nonzero;
- multiplication overflow fails;
- zero limit is invalid unless the API contract explicitly allows it and protocol semantics are defined;
- commit remains distinct from physical memory and swap;
- public label and JSON field use `commit` terminology.

### Required tests

- ordinary commit use;
- zero commit;
- full commit limit;
- total greater than limit;
- page size zero;
- multiplication overflow;
- API failure;
- large but valid values.

### Workstream F acceptance criteria

- [ ] Commit bytes use checked page conversion.
- [ ] Commit is serialized as commit.
- [ ] Invalid totals/limits fail.
- [ ] No pagefile/swap conflation exists in code, tests, or docs.

## Workstream G: construct v2 collected metrics and capabilities

The Windows collector returns the internal normalized sample expected by the sampler/server.

Required capability state:

```text
cpu_iowait: false
load_average: false
swap: false
memory_commit: true
```

Required values:

```text
cpu usage: present after warm-up
cpu iowait: absent
load: absent
memory: present
swap: absent
commit: present
```

The conversion layer must reject any contradiction before publication.

Do not make Windows produce a v1 `StatusSnapshot`. The Windows daemon should expose v2 endpoints and may return `404` for `/v1/status`, documented clearly.

### Workstream G acceptance criteria

- [ ] Windows capability/value pairs pass v2 validation.
- [ ] Unsupported metrics are `None`, not zero-filled.
- [ ] Windows has no v1 snapshot conversion.
- [ ] Sampler readiness behavior is identical in structure to other collectors.

## Workstream H: integrate the collector into `greggd run`

Add:

```rust
#[cfg(target_os = "windows")]
type NativeCollector = greggd::collector::windows::WindowsCollector;
```

Ensure:

- the daemon binary compiles on Windows;
- the Tokio runtime/features support Windows networking and Ctrl-C;
- v2 server routes are available;
- v1 route behavior is explicit on Windows;
- default foreground execution uses the Windows collector;
- no service-manager assumptions are required for `greggd run`;
- startup logs report Windows OS/architecture without leaking private system details.

Unsupported service commands remain truthful until Phase 43. They must return `NotAvailable`, not no-op success.

### Workstream H acceptance criteria

- [ ] `greggd run` compiles and starts on Windows.
- [ ] Collector warms then serves a valid v2 snapshot.
- [ ] `/v1/status` behavior on Windows is documented and tested.
- [ ] Service commands do not falsely claim success before Phase 43.
- [ ] Linux/macOS binary selection remains correct.

## Workstream I: testing and native smoke

### Deterministic tests

Use fake sources for all raw API/error cases. Do not require live system values for edge-case tests.

### Native Windows tests

Add tests that call the real source and assert structural invariants only:

- identity nonempty;
- logical cores greater than zero and within supported topology;
- memory total greater than zero;
- memory used not greater than total;
- commit used not greater than limit;
- first sample warms;
- later sample becomes ready within a bounded interval;
- v2 snapshot validates;
- unsupported metrics remain absent.

Do not assert exact CPU/memory percentages on a shared CI runner.

### Foreground daemon smoke

Using a temporary config bound to `127.0.0.1` and an ephemeral/allocated port:

1. start `greggd run` as a child process;
2. poll health until ready or bounded timeout;
3. fetch `/v2/status`;
4. validate JSON through `gregg-protocol`;
5. assert Windows capabilities;
6. terminate with Ctrl-C or supported process signal;
7. assert bounded clean exit;
8. clean temporary files/processes.

### Workstream I acceptance criteria

- [ ] Edge cases use deterministic fake-source tests.
- [ ] Real-source native tests assert only stable invariants.
- [ ] Foreground daemon smoke reaches v2 ready state.
- [ ] Child process cleanup is reliable.
- [ ] No CI artifact is needed to prove the smoke.

## Workstream J: documentation and limitations

Update:

- README supported targets;
- platform notes;
- architecture collector documentation;
- v2 protocol examples;
- known limitations;
- security/private-network statement.

Document at minimum:

- Windows x86-64 baseline;
- foreground daemon support after this phase;
- service support arrives in Phase 43;
- no load average, I/O-wait, or swap on Windows;
- commit is displayed separately;
- any processor-group/logical-processor limit;
- Windows API collection does not invoke external commands.

### Workstream J acceptance criteria

- [ ] Windows metric semantics are documented accurately.
- [ ] Topology limitation is explicit if present.
- [ ] Foreground versus service support is distinguished.
- [ ] No unsupported feature is advertised.

## Required validation commands

On Windows PowerShell:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
cargo build -p greggd --release
cargo run -p greggd -- --help
```

Run the native source tests and foreground daemon smoke.

On Linux/macOS, run the full local validation to prove target gating and existing collectors remain correct.

## Phase acceptance criteria

Phase 42 is complete only when:

- [ ] A native Windows collector exists behind the shared collector boundary.
- [ ] Native API calls are contained and testable.
- [ ] Windows identity, logical cores, CPU, physical memory, and commit are implemented.
- [ ] CPU requires a valid delta and recovers correctly from reset/failure.
- [ ] Load, swap, and I/O-wait are absent and capability-declared.
- [ ] Commit is distinct from swap in code, protocol, UI input, tests, and docs.
- [ ] Unsupported large CPU topology fails clearly unless fully supported.
- [ ] `greggd run` starts natively on Windows and serves valid v2 status.
- [ ] Deterministic fake-source tests cover all numeric/error boundaries.
- [ ] A native foreground daemon smoke passes.
- [ ] Linux/macOS behavior remains green.
- [ ] No service implementation or release automation is smuggled into the phase.

## Evidence required for completion

Only:

- passing local/native Windows tests;
- concise foreground smoke output;
- passing Linux/macOS local checks;
- code and documentation diff.

Do not add retained metrics dumps, release artifacts, or qualification manifests.

## Handoff notes for a smaller implementation model

1. Create raw structs and fake-source tests before writing FFI.
2. Implement identity, memory, and commit first; they are single-sample metrics.
3. Implement CPU delta math entirely in safe Rust with exhaustive tests.
4. Add FFI wrappers only after safe normalization tests pass.
5. Detect unsupported processor topology before reporting CPU values.
6. Integrate with the sampler after the collector tests pass.
7. Add the foreground daemon smoke last.
8. Keep Windows service behavior explicitly unavailable until Phase 43.
9. Do not solve unsupported metrics by zero-filling.