# Plan 111: live-metrics compatibility, verification, and documentation closure

Status: complete; implementation corrections `efb18dc` and `29a2633` plus
documentation closure `fa4b022` landed on main after verification.

Depends on: Plans 107-110.

## Objective

Close the live CPU-frequency, disk-I/O, and network-throughput work with explicit mixed-version evidence, real Ubuntu runtime verification, existing native-platform CI, and documentation that matches the implemented semantics.

This is a closure plan, not a second implementation pass. Correct defects discovered by verification, but do not add unrelated monitoring features.

## Required compatibility matrix

Verify these combinations explicitly.

### New client -> v1-only daemon

Expected:

- existing v2-first/v1-on-404 fallback still succeeds;
- CPU/memory/load/swap behavior remains unchanged;
- CPU frequency absent;
- disk I/O absent;
- network absent;
- no NET row/panel is fabricated under the all-old-fleet policy;
- `e` retains historical drive behavior when v1 has no drives;
- `n` is a no-op for that system.

### New client -> pre-feature v2 daemon

Expected:

- old `StatusPayloadV2` deserializes with new optional fields absent;
- existing drive capacity remains available;
- no CPU-frequency suffix;
- no disk-I/O throughput fabricated;
- no network telemetry fabricated;
- client remains online/healthy.

### New client -> new daemon

Expected:

- each supported optional metric appears independently;
- one unsupported/failed optional metric family does not hide the others;
- disk/network rates are live and reset-safe;
- NET utilization uses link capacity where known.

### Mixed fleet

Use at least one legacy fixture endpoint and one new daemon in the same client session.

Expected:

- no protocol-version-specific renderer branches leak into output;
- CPU frequency appears only on hosts that provide it;
- NET normal-row/condensed-column fleet policy is deterministic;
- old hosts render unavailable only when a fleet-wide column/row is active because another host supports it;
- scrolling/selection/expansion stays correct across blocks of different effective detail height.

### Old client compatibility

Where practical, serialize a new v2 payload and feed it to a pre-feature client fixture/build or an equivalent serde-compatibility test proving unknown JSON fields are ignored.

Do not require maintaining old binaries in CI indefinitely. A fixture-level proof is sufficient if exact old-binary execution is awkward.

## Ubuntu live runtime verification

Run on the current Ubuntu host with a temporary loopback config and direct daemon lifecycle. Do not require systemd.

### Baseline

1. Build current workspace in the normal local profile.
2. Launch `greggd` directly with a temporary config.
3. Wait for `/v2/healthz` Ready and `/v2/status` valid payload.
4. Record the relevant host interfaces/block devices so observed behavior can be interpreted correctly.

### CPU frequency

If `/sys/devices/system/cpu/cpufreq/` is available:

- confirm `cpu_frequency_hz` is present and plausible relative to source policy values;
- exercise idle versus bounded CPU load and confirm the metric can change where the platform governor does so;
- do not require a specific frequency transition on fixed-frequency/VM hosts.

If CPUFreq is unavailable, record the metric as truthfully absent and do not fail the entire smoke solely for that host limitation.

### Disk throughput

Generate bounded temporary-file I/O on a known local filesystem using ordinary user-space tools already present on the host or a small test helper. Avoid destructive raw-device writes.

Verify:

- baseline idle rates are low/zero as expected;
- write activity raises aggregate W/s;
- read activity raises aggregate R/s where cache behavior permits observable device reads;
- rates return toward idle after activity stops;
- no absurd startup/reset spike appears;
- a mapped drive row receives throughput only if the association is trustworthy.

Do not treat page-cache suppression of physical reads as a bug; the metric is block-device I/O, not application read syscall throughput.

### Network throughput and capacity

Use safe local/network traffic available in the environment.

Verify:

- `lo` appears in detail when collected;
- loopback traffic changes loopback Rx/s and Tx/s;
- loopback traffic alone does not create/inflate aggregate physical link-capacity utilization;
- an ordinary active interface exposes link capacity where the kernel/driver provides it;
- bounded traffic over an eligible interface changes aggregate Rx/s/Tx/s and NET percentage;
- simultaneous Rx and Tx cannot make the percentage exceed 100% solely because the link is full duplex.

If the environment does not permit meaningful physical-interface traffic, fixture/unit coverage remains authoritative for saturation math; record the live limitation rather than inventing evidence.

### Restart/reset behavior

1. Stop the daemon through the existing direct control path.
2. Restart it against the same config.
3. Confirm cumulative disk/network source counters do not cause the first daemon sample to publish stale lifetime-average or enormous rates.
4. Confirm rates warm from fresh daemon baselines.

## TUI manual smoke

Against the same new daemon plus a legacy fixture endpoint:

- normal view shows CPU clock where available;
- NET appears under DISK according to the mixed-fleet policy;
- `e` shows disk aggregate and per-associated-row R/s/W/s;
- `n` shows aggregate and individual interfaces with Rx/s/Tx/s;
- loopback is visible in detail but not aggregate capacity;
- `v` shows the NET column at supported width tiers;
- resize wide -> narrow -> wide while `e` and/or `n` are expanded;
- navigate selection, page up/down, first/last;
- let Plan 087's selection highlight timeout fire and confirm logical selection/expansions remain intact.

## Native-platform CI expectations

Use the existing workflow only. Do not add a new matrix.

Existing native jobs must prove:

- macOS Intel/arm64 compile and unit tests for optional CPU-frequency absence, IOKit disk statistics wrappers, and interface-stat parsing;
- Windows compile and unit tests for `CallNtPowerInformation`, disk-performance bindings, and IP Helper interface data;
- MSRV Rust 1.75 remains green unless a separately approved project-wide decision changes it.

If new Windows API bindings require feature flags/dependencies, ensure those are target-scoped and compatible with the workspace's dependency policy.

## Performance/overhead sanity

Gregg is intended for lightweight local deployment, including SBCs.

Measure/inspect enough to ensure the new one-second live telemetry does not accidentally introduce obvious overhead such as:

- spawning processes each sample;
- rescanning expensive filesystem topology unnecessarily each sample;
- opening unbounded numbers of handles/files without reuse/limits;
- blocking the async HTTP runtime with slow native enumeration outside the existing blocking collector boundary;
- payload growth without entry bounds.

A microbenchmark suite is not required. A simple before/after daemon idle CPU/RSS observation on Ubuntu plus code-path review is sufficient unless a regression is visible.

Do not optimize away correctness for tiny theoretical savings.

## Documentation reconciliation

Update all applicable current documentation, not historical completed-plan records.

At minimum review:

```text
README.md
CHANGELOG.md
crates/gregg/README.md
crates/greggd/README.md
docs/client.md
docs/display.md
docs/daemon.md
architecture/protocol.md
architecture/gregg-client.md
architecture/greggd-daemon.md
.opencode/skills/gregg-client/SKILL.md
.opencode/skills relevant to greggd collector/protocol work
plans/README.md
```

Document clearly:

- CPU frequency is optional and means current/OS-reported frequency, not base/max frequency;
- macOS may omit current CPU frequency because Gregg does not use privileged/undocumented mechanisms;
- R/s/W/s and Rx/s/Tx/s are byte throughput rates;
- disk capacity and disk I/O are distinct accounting domains;
- network percentage is link-capacity utilization using directional full-duplex-safe math;
- loopback may be displayed but is excluded from aggregate capacity;
- older daemons remain supported and simply lack newer optional telemetry;
- `e`, `n`, and `v` key behavior.

Do not document daemon fleet-version tracking; it remains deferred.

## Plan-record reconciliation

After implementation closes:

- mark Plans 107-111 with truthful status/implementation SHAs;
- record the Ubuntu runtime smoke observations and relevant environment limitations;
- record the existing CI run ID that validates Linux/macOS/Windows/MSRV;
- update `plans/README.md` with concise current-direction and plan-table entries;
- do not rewrite historical completed plans to pretend they originally included these metrics.

## Mandatory local verification

Run:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
```

Also run any repository-standard MSRV/local packaging checks that `check-local.sh` intentionally delegates elsewhere if the current plan index/AGENTS instructions require them.

No new release automation or CI infrastructure is part of this work.

## Defect correction policy during closure

If verification finds a defect directly caused by Plans 108-110, correct it in the owning module before marking closure.

Examples in scope:

- old-v2 fixture no longer deserializes;
- network percentage exceeds 100% because Rx+Tx were summed;
- bond slave capacity counted twice;
- first sample emits a huge disk rate;
- `n` expansion breaks page navigation;
- Windows/macOS build failure caused by incorrect cfg/imports.

Examples out of scope unless blocking:

- process-level network usage;
- temperature/power telemetry;
- historical sparklines;
- wireless signal strength;
- disk latency/IOPS columns beyond the requested throughput;
- daemon fleet-version inventory/update UI.

Create a later plan for those rather than broadening this closure.

## Acceptance criteria

Plan 111 and the Plan 107 roadmap are complete only when:

1. New-client/v1-only compatibility is demonstrated.
2. New-client/pre-feature-v2 compatibility is demonstrated with old payload fixtures.
3. New-client/new-daemon telemetry works with independent optional degradation.
4. A mixed old/new fleet renders coherently without wire-version renderer branches.
5. Unknown new JSON fields remain backward-compatible for older clients by serde/fixture evidence.
6. Ubuntu direct-runtime smoke demonstrates bounded live CPU-frequency behavior where supported, disk throughput, network detail/capacity behavior, and clean restart re-baselining.
7. Loopback detail is visible without contaminating physical aggregate capacity.
8. Full-duplex utilization math cannot produce >100% merely from simultaneous Rx and Tx.
9. TUI `e`, `n`, `v`, resize, selection, and viewport behavior is manually and automatically verified.
10. New collection remains bounded and lightweight enough for Gregg's SBC/local-server scope.
11. Existing Linux/macOS/Windows/MSRV CI is green without adding a new matrix.
12. Current docs/architecture/skills describe the implemented semantics accurately.
13. Plans 107-111 and `plans/README.md` carry truthful closure records.
14. Daemon fleet-version transport/display remains deferred and unimplemented.

## Closure record

Plan 111 is complete. The final verification found and corrected one defect
directly in the Plan 109 Linux CPUFreq path: this Ubuntu host exposes
`affected_cpus` as a whitespace-separated list, so the parser now accepts
kernel range, comma-separated, and whitespace-separated forms. The regression
is covered by `collector::linux::source::tests::cpufreq_accepts_kernel_space_separated_cpu_lists`.
The same verification also corrected the sustained-workload compatibility
driver so valid `OnlineV2` results count as online alongside v1 results.

Compatibility evidence on the current tree:

- `gregg-protocol` integration tests round-trip v1, legacy v2, and live-metrics
  fixtures; v1 and old-v2 normalization leaves CPU frequency, disk I/O, and
  network absent; additive subsets/nulls deserialize; older v2 snapshot models
  ignore future live fields.
- Client normalization and renderer tests cover independent optional families,
  exact full-duplex directional utilization, loopback detail, mixed old/new
  fleet geometry, `e`/`n`/`v`, resize, selection, and mixed-height viewport
  behavior without wire-version renderer branches.
- The Ubuntu 24.04.4 LTS aarch64 host smoke used a temporary loopback config
  and direct `greggd` lifecycle. CPUFreq was present but `cpuinfo_cur_freq`
  was permission-denied; readable `scaling_cur_freq` then reported changing
  current values (1.7–2.2 GHz) after the parser correction. `/sys/block`
  exposed `mmcblk0` and `nvme0n1` after loop/RAM filtering. Bounded temporary
  file writes and direct reads raised W/s and R/s, then rates returned to zero.
  `lo` reported loopback traffic in detail while `eth0` supplied 1 Gb/s
  aggregate capacity; loopback did not inflate the physical aggregate. A
  physical-interface traffic generator was not available in this environment,
  so deterministic directional/topology tests remain authoritative for
  full-duplex saturation and capacity aggregation.
- Direct control-socket stop and restart succeeded. The first post-restart
  status had no disk/network rate fields, and the following sample warmed to
  ordinary rates without a lifetime-average spike.
- The existing PTY mixed-fleet TUI smoke exercised a current daemon together
  with a v1-only fixture through normal/condensed views, `e`/`n`, resize, and
  navigation. Automated renderer tests provide the stable assertions for the
  NET row/column and detail labels because terminal redraw escape sequences are
  not a durable artifact of this noninteractive runner.

Required local gates passed on the final implementation tree:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
rustup run 1.75 cargo check --workspace --all-features
```

The documentation build emits only the repository's pre-existing rustdoc link
warnings. Existing native-platform CI remains the authority for macOS
Intel/arm64, Windows API bindings and SCM behavior, and MSRV 1.75; no new
matrix or workflow was added. Daemon fleet-version transport/display remains
deferred and unimplemented.
