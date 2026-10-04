# Plan 167: Cron/client-daemon correctness and footprint closure

Status: planned.

Depends on: completed Plans 163, 165, and 166. Independent of Plan 091.

## Objective

Close the Plan-161 line with end-to-end evidence that cron observability and
the Gregg client-daemon split are correct, bounded, cross-platform, and still
consistent with Gregg's small local-monitor character.

This is not a feature-expansion plan. Correct concrete defects found during the
qualification, but open a new corrective plan for any change that materially
alters the settled architecture.

## End-to-end scenarios

Demonstrate the following with deterministic integration tests or focused local
smokes, using the lightest appropriate mechanism.

### One remote polling plane

Start one client daemon and two local TUI/test subscribers against the same
config.

Prove:

- endpoint request count follows one configured Systems cadence, not two;
- scheduler-summary request count follows one daemon cadence;
- scheduler-history request occurs only on discovery/revision change;
- EggPool has one worker/request plane;
- closing one subscriber has no polling effect;
- closing the final subscriber does not stop the daemon.

This is the primary architectural proof for the split.

### Lazy activation race

With no daemon running, launch two attach clients concurrently.

Prove:

- one config-specific daemon is started;
- both clients attach to it;
- no orphan second daemon remains;
- failed readiness produces bounded actionable failure.

### Remote scheduler lifecycle

Exercise one load-gated test job through:

~~~text
scheduled
-> high-load delayed
-> retry
-> eligible
-> running
-> terminal success/failure
~~~

The client/TUI state must never claim running before launch or success before
terminal status exists.

Also exercise max-wait expiry without a child.

### Output flood

Run a child that writes far beyond both stdout/stderr caps.

Prove:

- child does not block on full pipes;
- scheduler remains responsive;
- retained output equals the qualified bounded tail policy;
- truncation flags render;
- daemon memory does not grow with total bytes written;
- shutdown stays within the established bounds.

### Restart matrix

Cover independently:

- TUI restart while client daemon stays running;
- client-daemon restart while remote greggd stays running;
- remote greggd restart while client daemon stays running;
- updated Gregg binary encountering an old owned client daemon;
- one remote old/pre-scheduler greggd beside one new greggd.

Expected semantics:

- TUI restart loses only presentation state;
- client-daemon restart loses its extended local-only history but reseeds from
  remote retained history;
- greggd restart loses remote memory history/current pending state, while the
  client may retain previously observed local records under its bounds and
  starts a new remote epoch;
- no history duplication across epoch/sequence boundaries;
- old daemon remains online with cron unsupported.

## Memory qualification

Record measured/derived memory costs for both processes.

### greggd

From the exact Plan-162 constants calculate:

- zero-job scheduler history overhead;
- default 5-record x 64-job maximum retained-output allocation;
- configured-maximum history allocation.

Measure RSS/allocator behavior if practical with representative noisy output.
The implementation must remain bounded even if allocator RSS does not instantly
return after ring eviction.

### Gregg client daemon

Calculate/measure:

- base fleet state;
- local cron cache default;
- configured maxima/global ceiling;
- one versus multiple attached frontends.

A second TUI should add only connection/frontend-buffer overhead, not another
fleet cache or HTTP client/poller.

If the default local cron cache creates a disproportionate memory increase,
reduce the cache/output retransmission design before closing.

## CPU/network qualification

At idle/default cadence record:

- greggd scheduler with configured jobs but nothing due;
- client daemon with one representative fleet;
- client daemon with zero attached TUIs;
- same daemon with two attached TUIs.

No busy frame loop or IPC heartbeat should be introduced solely to keep the TUI
current.

The Plan-160 one-minute civil reconciliation wake remains scheduler-internal
and should not trigger HTTP/history publication if state is unchanged.

## Payload/body qualification

Measure representative and maximum-shaped:

- /v2/scheduler;
- /v2/scheduler/history;
- local daemon frontend summary messages;
- local on-demand history response if Plan 166 selected a two-tier IPC model.

Confirm configured client body caps exceed valid maxima with bounded margin and
reject oversized/invalid responses cleanly.

## Binary/dependency footprint

Reproduce the same stripped target/profile baselines used by the preceding
plans.

### greggd

Compare to:

~~~text
3,261,664 bytes
~~~

and Plan 162's numeric rule.

If Plan 163 exceeded its allowed rule but was not corrected, this plan cannot
silently close it. Either remove the excess or open an explicit re-baseline
decision plan with measured component attribution.

### gregg

Record stripped client size before and after:

- local IPC/client-daemon core;
- lifecycle manager support;
- scheduler observability/TUI.

Attribute any material jump to dependencies/features versus application code.
Prefer existing Tokio/windows-sys/libc/serde primitives; remove accidental
feature widening.

## Cross-platform native truth

Use existing CI jobs to prove:

- Linux build/tests plus Unix socket/lifecycle helpers;
- macOS arm64/Intel build/tests plus LaunchAgent/native socket paths where
  testable;
- Windows release build/tests plus named-pipe/security/startup helpers;
- Rust 1.89 MSRV.

Do not add a second native matrix for ceremony. Add only narrowly necessary
native smoke to the existing jobs if pure tests cannot establish a platform
contract.

## Security review

Verify the line did not accidentally add a control plane.

Check:

- scheduler HTTP routes are GET/HEAD only;
- no job mutation/start/cancel endpoint;
- command argv/working-dir not exposed unless explicitly qualified;
- bounded stdout/stderr only;
- documentation clearly states output listener trust implications;
- Unix local socket mode/ownership;
- Windows same-user pipe access;
- foreign local endpoint fails closed;
- output sanitizer covers terminal controls;
- no internal sudo/setuid/credential store;
- service/user lifecycle stays per-user for Gregg clientd.

## Config/update/install regression

Run focused regression coverage for:

- existing gregg add/list/remove/refresh/edit;
- Ctrl-R last-known-good behavior;
- bootstrap install PATH semantics;
- binary-first update staging/checksum/version verification;
- user daemon restart after update;
- uninstall dry-run/purge;
- sibling greggd untouched by Gregg client uninstall;
- explicit config isolation.

Cron/clientd additions must not regress the settled component-safe lifecycle
work.

## Documentation reconciliation

Review active docs for stale statements that Gregg itself owns remote polling.

At minimum reconcile:

- README.md;
- crates/gregg/README.md;
- crates/greggd/README.md;
- AGENTS.md;
- CHANGELOG.md;
- architecture/overview.md;
- architecture/gregg-client.md;
- architecture/greggd-daemon.md;
- architecture/protocol.md;
- architecture/gregg-protocol.md;
- architecture/scripts-and-packaging.md;
- relevant OpenCode skills;
- plans/README.md;
- Plan 161 closure record.

Do not rewrite historical plans whose old architecture was correct at the time.
Append correction/closure notes where necessary.

## Verification set

Use focused tests plus:

~~~text
./scripts/check-local.sh
~~~

Use ./scripts/check-local.sh --release because this line touches packaging,
update/install behavior, binary footprint, and user-visible release assets.

Run existing CI once on the final implementation SHA when native-platform truth
is required. Record the exact run ID.

No dedicated evidence workflow/artifact bundle.

## Acceptance criteria

- [ ] Two TUIs share one Systems polling plane.
- [ ] Two TUIs share one scheduler polling/history plane.
- [ ] Two TUIs share one EggPool worker.
- [ ] Last TUI disconnect does not stop clientd.
- [ ] Simultaneous first attach produces one client daemon.
- [ ] Load-high and load-unavailable delays are visible and truthful.
- [ ] Max-wait expiry appears as a terminal cron record.
- [ ] Output flood remains bounded and cannot deadlock child execution.
- [ ] TUI output sanitizer blocks terminal-control injection.
- [ ] TUI restart preserves daemon-side local history.
- [ ] Client-daemon restart reseeds from remote history without duplication.
- [ ] Remote greggd restart starts a new deduplication epoch cleanly.
- [ ] Old greggd remains online with cron unsupported.
- [ ] Default/maximum greggd history memory is recorded.
- [ ] Default/maximum client cache memory is recorded and globally bounded.
- [ ] Multiple frontends do not duplicate fleet cache memory materially.
- [ ] Scheduler/history HTTP bodies respect Plan-162 limits.
- [ ] Local IPC messages are bounded.
- [ ] greggd footprint meets Plan-162 rule or an explicit follow-up decision is
      opened.
- [ ] gregg binary/dependency growth is measured and attributed.
- [ ] Linux/macOS/Windows/MSRV existing CI is green as applicable.
- [ ] Update/install/uninstall/config regressions are green.
- [ ] User-visible and architecture docs are reconciled.
- [ ] Plan 161 can be truthfully marked complete with no hidden persistence,
      remote control, or duplicate-polling path.

## Stop conditions

Do not close Plan 161 if:

- direct TUI polling remains an ordinary silent fallback;
- a second TUI causes a second remote polling cadence;
- history/output memory is not strictly bounded;
- client daemon exits when the last TUI closes;
- local IPC is accessible beyond the intended user;
- update can leave the selected owned daemon knowingly protocol-incompatible;
- scheduler output can inject terminal controls;
- a scheduler footprint overrun is left as an undocumented new baseline;
- active documentation still describes the old TUI-owned polling architecture.

## Handoff

If all criteria pass, append the exact implementation SHA, measurements, and
existing CI run ID to this plan and Plan 161, mark Plans 161-167 complete as
appropriate, and register the closed roadmap group in plans/README.md.
