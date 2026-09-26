# Plan 137: native telemetry closure and FreeBSD network-evidence corrective pass

Status: complete.

Depends on: completed Plans 132-136 at the current post-`e2dba59` main state. This work is independent of the remaining Plan 091 soak record.

## Objective

Close the remaining record/evidence defects found after the native host telemetry extraction campaign without reopening the extraction architecture or changing Gregg's supported metric semantics.

This corrective pass owns four narrow findings:

1. Plans 132-136 are marked complete and their closure records state that acceptance holds, but all 67 acceptance checkboxes remain unchecked.
2. The Plan-136 FreeBSD loopback qualification test can pass without generating traffic, can pass without matching a loopback interface before/after, and currently proves only non-decreasing counters rather than actual traffic-driven advancement.
3. Plan 136 overstates what the loopback test demonstrates by calling it a byte-field "direction" proof. Loopback traffic cannot independently establish RX-vs-TX field ordering because the same local exchange legitimately advances both directions.
4. `gregg-host` package metadata says "(and BSD)" even though the implemented/qualified fourth backend is specifically FreeBSD and NetBSD/OpenBSD remain deferred.

The plan also reconciles the duplicated `d928950` entry in the Plans 132-136 registry implementation lists.

No native collector formula, public API, protocol mapping, daemon readiness behavior, sampling cadence, platform support claim, or production subprocess policy changes in this pass.

## Finding 1: Plans 132-136 closure records contradict their checklists

Current state:

- Plan 132: `Status: complete`, 10 unchecked acceptance boxes.
- Plan 133: `Status: complete`, 11 unchecked acceptance boxes.
- Plan 134: `Status: complete`, 14 unchecked acceptance boxes.
- Plan 135: `Status: complete`, 17 unchecked acceptance boxes.
- Plan 136: `Status: complete`, 15 unchecked acceptance boxes.

Each plan has a closure record and the registry marks the campaign complete. The unchecked boxes are therefore record drift, not an implementation request.

### Required correction

Reconcile each Plan 132-136 acceptance section against the actual closure evidence.

For every acceptance item:

- mark it checked only when the current implementation/record supports it;
- if an item was intentionally satisfied by a truthful unsupported result (for example FreeBSD swap remaining unsupported pending `kvm_getswapinfo` qualification), keep the item checked only when the original criterion explicitly permits that result;
- if any acceptance item is not actually supported, leave it unchecked and make Plan 137 own the missing correction instead of asserting completion;
- do not rewrite implementation SHAs or historical CI evidence;
- do not alter the historical rationale of Plans 132-136.

The intended outcome is that status, checklist, closure prose, and registry all agree.

## Finding 2: FreeBSD loopback native test is fail-open

Current `native_loopback_traffic_advances_lo_counters()` behavior is too weak for the Plan-136 closure claim:

~~~rust
if !ping.is_ok_and(|output| output.status.success()) {
    eprintln!("ping unavailable; skipping loopback direction proof");
    return;
}

if let (Some(previous), Some(current)) = (lo_before, lo_after) {
    assert!(
        current.rx_bytes >= previous.rx_bytes &&
        current.tx_bytes >= previous.tx_bytes
    );
}
~~~

This permits a green test when:

- `ping` cannot be executed;
- `ping` fails;
- no loopback record exists before traffic;
- no matching loopback record exists afterward;
- both counters remain exactly unchanged.

That means CI run `36220930632` proves the test executed and passed, but the test itself does not establish the stronger traffic-activity claim recorded in Plan 136.

### Required implementation

Strengthen the FreeBSD-only native test so every qualification precondition is mandatory on the supported CI image.

At minimum:

1. read native ifmib records before traffic;
2. require at least one loopback record before traffic;
3. select a stable loopback identity and preserve it for the after-sample match;
4. invoke the FreeBSD base-system ping command and require successful execution/status;
5. read native ifmib records after traffic;
6. require the same loopback identity to still be present;
7. require both byte counters to be monotonic;
8. require at least the expected traffic-relevant byte counters to advance strictly; for the loopback ping qualification, require both RX and TX to increase unless native FreeBSD evidence demonstrates a different valid accounting behavior on the supported floor;
9. retain a bounded sleep/retry only if necessary for accounting visibility; do not turn this into an unbounded/flaky polling loop;
10. fail rather than print-and-return when any qualification prerequisite is absent on the pinned FreeBSD CI image.

The native qualification job is intentionally tied to an ordinary FreeBSD VM topology. If the pinned image unexpectedly lacks the base ping utility or a loopback interface, that is a qualification-environment failure and should be visible rather than silently skipped.

Do not add a production dependency or production subprocess. The command is test-only traffic generation, consistent with the existing disk-write qualification using the base `sync` utility.

## Finding 3: separate counter-activity evidence from RX/TX field-order evidence

Correct Plan 136 and any current architecture/README wording that calls the loopback traffic test a directional-field proof.

A loopback request/reply exchange is useful native evidence for:

- the selected ifmib row being real;
- the mapped byte-counter fields being live;
- counters being monotonic;
- generated loopback traffic causing observable byte-count advancement.

It does **not** independently prove that one mapped field is RX and the other is TX, because loopback traffic advances both directions.

### Required evidence wording

Use precise language such as:

> The native loopback smoke proves that the mapped ifmib byte-counter fields are live and advance under known loopback traffic. RX/TX semantic ordering is grounded in the field-for-field FreeBSD `struct if_data` ABI mapping, not inferred from symmetric loopback traffic.

Do not claim asymmetric direction proof unless an actual asymmetric qualification is added.

No new asymmetric test is required for Plan 137 unless implementation discovers that the current ABI mapping itself is uncertain. The corrected `IfDataPrefix` layout plus native activity proof is sufficient for this bounded pass.

### Optional stronger cross-check

If a deterministic, low-maintenance cross-check against a native reference can be added without production dependencies or fragile parsing, it may be used as supplementary evidence. It is not required and must not expand this pass into a general `netstat` parser or shell-based production telemetry source.

## Finding 4: crate metadata overstates BSD support

Change the `crates/gregg-host/Cargo.toml` description from generic "(and BSD)" wording to explicit FreeBSD wording.

The package metadata should describe support as Linux, macOS, Windows, and FreeBSD.

Keep the README's existing precise statement:

- FreeBSD `x86_64-unknown-freebsd` natively qualified;
- FreeBSD aarch64 compile support as currently evidenced;
- full `greggd` FreeBSD service/install/release support deferred;
- NetBSD/OpenBSD future backends, not implied.

Do not add `bsd` as a cargo feature or generic portability claim.

## Registry reconciliation

Update `plans/README.md` so:

- Plan 136 is shown as complete with a Plan-137 corrective follow-up;
- Plan 137 is registered as planned/in implementation until it closes;
- the dependency chain becomes `... -> 136 -> 137`;
- the duplicated trailing `d928950` in the Plans 132-136 implementation lists is removed;
- the cumulative implementation list remains historically accurate;
- Plan 137 is recorded as independent of Plan 091.

When Plan 137 closes, register its implementation SHA and the exact CI run proving the corrected native FreeBSD test.

Do not rewrite the original Plan-136 CI run `36220930632`; retain it as valid evidence for the implementation state it tested, with Plan 137 explicitly correcting the strength of the network-evidence claim.

## Verification

Run the focused tests first.

On ordinary development hosts:

~~~text
cargo test -p gregg-host --all-targets --all-features
cargo test -p greggd --all-targets --all-features -- collector
~~~

Then run the standard repository gates:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

The final implementation SHA requires one ordinary CI run with all current jobs green:

- Linux;
- macOS arm64;
- macOS Intel;
- Windows including SCM smoke;
- MSRV Rust 1.89;
- FreeBSD 14.2 `gregg-host` native qualification.

The FreeBSD job must show the strengthened loopback test running and passing; a skip/early-return path is not acceptable.

No new workflow, runner, matrix, artifact bundle, or privileged environment is required.

## Acceptance criteria

- [x] Plans 132-136 acceptance checklists are reconciled item-by-item with their existing closure evidence.
- [x] No unsupported Plan-132-136 acceptance item is checked merely to make the records look complete.
- [x] Plan 136 receives an appended Plan-137 correction note rather than rewritten historical implementation/CI evidence.
- [x] `native_loopback_traffic_advances_lo_counters` cannot pass when ping execution fails.
- [x] The loopback native test requires a loopback interface before traffic and the same stable interface after traffic.
- [x] The loopback native test retains monotonicity checks and requires strict traffic-driven byte-counter advancement.
- [x] The FreeBSD native test remains bounded and deterministic enough for the pinned FreeBSD 14.2 VM job.
- [x] Documentation no longer claims symmetric loopback traffic independently proves RX-vs-TX field ordering.
- [x] RX/TX semantic ordering is attributed to the field-for-field FreeBSD `struct if_data` ABI mapping; native traffic is described as counter-activity evidence.
- [x] The existing disk-write native proof is not weakened.
- [x] `gregg-host` Cargo package description names FreeBSD explicitly rather than generic BSD support.
- [x] NetBSD/OpenBSD remain explicitly deferred and are not implied by metadata.
- [x] The duplicate `d928950` registry entry is removed without changing historical implementation meaning.
- [x] Plan 136 remains complete with Plan 137 recorded as its narrow corrective follow-up.
- [x] Existing Linux/macOS/Windows collector behavior, Gregg v1/v2 protocol behavior, readiness, cadence, slow-probe isolation, dependency boundary, and release footprint are unchanged.
- [x] One ordinary final CI run is green across Linux, both macOS jobs, Windows, MSRV 1.89, and the strengthened FreeBSD native job.
- [x] Plan 137 closure records the final implementation SHA and exact CI run.

## Explicit non-goals

Do not include:

- changing FreeBSD CPU, memory, filesystem, disk-I/O, or network formulas;
- adding FreeBSD swap or CPU-frequency support;
- changing ifmib/devstat production sources unless the strengthened qualification exposes a concrete defect;
- adding NetBSD/OpenBSD backends;
- making the full `greggd` product support FreeBSD;
- rc.d/service/installer/release-binary work;
- protocol-v3 or new protocol fields;
- client/TUI changes;
- sampler cadence or clock-ownership changes;
- Windows processor-group work;
- generic Unix/BSD abstractions;
- production shell/command scraping;
- new dependencies solely for test snapshots or ABI assertions;
- reopening the completed extraction architecture.

## Handoff note

Start by tightening `crates/gregg-host/src/freebsd/tests.rs::native_loopback_traffic_advances_lo_counters`.

Do not paper over a failing native assertion by restoring a skip path. If the strengthened test exposes a real ifmib parsing defect, correct that defect narrowly and record it in Plan 137. Otherwise keep this pass limited to qualification truthfulness, metadata, and planning-record reconciliation.

## Closure record

Implemented at `f5c2c4c` with remote CI run `36223217199` green across
Linux, macOS arm64, macOS Intel, Windows (incl. SCM smoke), MSRV Rust
1.89, and the strengthened FreeBSD 14.2 `gregg-host` native
qualification.

Finding 1: all 67 Plans 132-136 acceptance boxes reconciled item by
item against closure evidence and marked checked (132: 10, 133: 11,
134: 14, 135: 17, 136: 15). Every item holds at the implementation
SHA, including the intentionally truthful unsupported results the
original criteria explicitly permit (FreeBSD swap remaining
unsupported pending `kvm_getswapinfo` qualification; CPU frequency
unsupported without a validated source). No box was checked merely to
look complete; no implementation SHA or historical CI evidence was
rewritten; historical rationale preserved.

Finding 2: `native_loopback_traffic_advances_lo_counters` is now
fail-closed on the pinned FreeBSD image. It requires a loopback record
before traffic, preserves the stable loopback identity (id plus name)
for the after-sample match, requires base-system ping execution and
successful status, requires the same identity still present afterward
with monotonic counters, and requires both RX and TX to advance
strictly (one bounded ~1s visibility re-read only). Any missing
prerequisite fails rather than printing and returning. The FreeBSD job
in run `36223217199` shows the test running and passing
(`native_loopback_traffic_advances_lo_counters ... ok`, 25 passed, 0
failed); no skip path remains. No production dependency or subprocess
was added; the disk-write proof is unchanged.

Finding 3: loopback evidence wording corrected to counter-activity.
The test doc, `freebsd/source.rs` ifmib comment,
`architecture/collectors.md` FreeBSD section, and the appended Plan-136
correction note all state: the native loopback smoke proves that the
mapped ifmib byte-counter fields are live and advance under known
loopback traffic; RX/TX semantic ordering is grounded in the
field-for-field FreeBSD `struct if_data` ABI mapping, not inferred
from symmetric loopback traffic. No asymmetric direction claim is
made; no new asymmetric test was needed as the ABI mapping stands.
Plan 136 keeps its original implementation/CI evidence with the
correction appended, not rewritten.

Finding 4: `crates/gregg-host/Cargo.toml` now names FreeBSD
explicitly (Linux, macOS, Windows, and FreeBSD); no `bsd` feature or
generic portability claim was added. NetBSD/OpenBSD remain deferred;
the crate README's precise support statement is unchanged.

Registry: the duplicated trailing `d928950` is removed from all five
Plans 132-136 index entries without changing historical meaning; the
chain is `... -> 136 -> 137`; Plan 136 is complete with Plan 137 as
its narrow corrective follow-up. Original Plan-136 CI run
`36220930632` is retained as valid evidence for the state it tested.

Verification: focused `gregg-host` (24 passed locally) and
`greggd collector` (62 passed) suites green; `cargo fmt --check`,
workspace clippy `-D warnings`, and `./scripts/check-local.sh` green
(one unrelated flaky `mixed_fleet_evidence` client timing failure
passed on deterministic rerun). No collector formula, API, protocol,
readiness, cadence, platform-support, or production-subprocess change.

Acceptance: all boxes hold at the implementation SHA. Independent of
the remaining Plan 091 soak record; no downstream plan status changes
required — Plan 137 is terminal in the 132-137 chain and no future
plan depends on it.
