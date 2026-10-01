# Plan 149: FreeBSD CI VM bootstrap reliability corrective pass

Status: planned.

Depends on: completed Plans 136-137 and the current six-job CI baseline. Independent of the remaining Plan 091 soak record and Plans 147-148.

## Objective

Make the existing FreeBSD native `gregg-host` qualification job fail boundedly and run reliably without changing Gregg's FreeBSD telemetry implementation, supported metric semantics, or the pinned FreeBSD 14.2 qualification floor.

The current defect is in CI infrastructure, not in the collector. GitHub successfully assigns an `ubuntu-latest` runner and completes source checkout, but the nested FreeBSD VM intermittently never becomes SSH-ready inside `vmactions/freebsd-vm@v1.1.9`. The action's legacy readiness loop then prints `VM is booting` every approximately two seconds until GitHub's six-hour workflow limit cancels the job.

This plan replaces that legacy action line with the current maintained action, adds a repository-owned timeout backstop, cancels superseded same-ref FreeBSD jobs, and removes unnecessary VM-to-host build-tree copyback. It deliberately preserves the native FreeBSD 14.2 test command and all collector/product behavior.

## Diagnostic evidence

The 2026-10-01 review established the failure boundary from GitHub Actions job records and logs.

Two completed examples demonstrate the unbounded failure mode:

- workflow run `36459747600`, FreeBSD job `109055044454`: the VM domain was created successfully, `First boot` was reached, then `waitForVMReady` emitted approximately 10,653 `VM is booting` probes until the job was cancelled at the six-hour boundary;
- workflow run `36455420240`, FreeBSD job `109040415687`: the same boundary repeated with approximately 10,652 readiness probes before six-hour cancellation.

A healthy comparison is workflow run `36872317548`, FreeBSD job `110402922579`:

- GitHub runner setup and checkout succeeded normally;
- the same FreeBSD action step completed successfully in about two minutes;
- the VM readiness phase emitted only about 16 `VM is booting` probes before becoming reachable;
- `cargo test -p gregg-host --all-features` completed successfully.

The defect is therefore intermittent nested-VM bootstrap/readiness failure after libvirt/QEMU domain creation, not GitHub runner scheduling and not a deterministic Gregg test failure.

The diagnosed `vmactions/freebsd-vm@v1.1.9` path uses the older `vmactions/vbox` helper and delegates readiness to an effectively unbounded `waitForVMReady` loop. It also targets Node 20 metadata and is currently forced by GitHub-hosted runners to execute under Node 24.

As of 2026-10-01, upstream `vmactions/freebsd-vm@v1.5.8` is the maintained current line. Its action metadata uses Node 24 directly, its implementation has moved to the newer AnyVM machinery, and its published support table still includes FreeBSD 14.2 on x86_64. The action also supports `copyback: false`; `usesh` is deprecated and ignored because prepare/run scripts are already executed with `sh`.

## Scope decisions

This corrective pass owns only CI reliability around the existing FreeBSD native qualification.

Preserve all of the following:

- `runs-on: ubuntu-latest`;
- FreeBSD release `14.2` as the exact qualification floor;
- native `gregg-host` only; do not expand the full `greggd` product to FreeBSD;
- the existing test command `cargo test -p gregg-host --all-features`;
- the existing `prepare` installation of `curl` and `ca_root_nss`;
- stable Rust installed inside the guest through the current rustup path;
- existing Linux, both macOS, Windows, and MSRV jobs unchanged;
- Plans 136-137 as the historical implementation and evidence record for the FreeBSD collector itself.

The correction should make CI infrastructure failure visible quickly and specifically. It must not hide a real FreeBSD collector/test failure with `continue-on-error`, conditional skipping, or a non-blocking job.

## Implementation

### A. Upgrade the FreeBSD VM action to the current maintained line

In `.github/workflows/ci.yml`, replace:

~~~yaml
uses: vmactions/freebsd-vm@v1.1.9
~~~

with:

~~~yaml
uses: vmactions/freebsd-vm@v1.5.8
~~~

Update the adjacent provenance comment so it no longer describes 1.1.9 as the selected maintained action.

Retain:

~~~yaml
release: "14.2"
~~~

Do not opportunistically move the qualification floor to 14.3, 14.4, 15.x, or a floating `14` selector. FreeBSD 14.2 is part of the current Gregg portability evidence and remains supported by the upstream 1.5.8 action.

Remove the obsolete:

~~~yaml
usesh: true
~~~

input. In the current upstream action it is deprecated and ignored; prepare/run scripts execute with `sh` already.

Do not add `debug-on-error` or VNC/tmate behavior to normal CI.

### B. Bound the complete FreeBSD job

Set a job-level timeout:

~~~yaml
timeout-minutes: 20
~~~

The bound intentionally covers runner-side VM dependency setup, image acquisition/import, boot/readiness, source synchronization, rustup, and the native `gregg-host` test.

Twenty minutes is deliberately above the observed healthy distribution: recent successful jobs are commonly about two to three minutes, while the slowest observed healthy example in the diagnostic window was about eleven minutes. The timeout therefore leaves substantial operational margin while reducing the current worst case from six hours to twenty minutes.

Do not rely solely on upstream internal timeout behavior. Gregg owns this outer CI budget even if the action later changes its bootstrap implementation.

If implementation evidence shows a legitimate healthy 14.2 qualification can exceed twenty minutes under ordinary GitHub-hosted conditions, record the evidence and raise the bound narrowly. Do not remove the bound.

### C. Cancel superseded same-ref FreeBSD jobs

Add job-level concurrency for this job only:

~~~yaml
concurrency:
  group: gregg-ci-freebsd-${{ github.ref }}
  cancel-in-progress: true
~~~

This prevents multiple stale FreeBSD VM jobs for the same branch/ref from simultaneously consuming hosted-runner time after rapid pushes.

Keep concurrency scoped to the FreeBSD job. Do not cancel or serialize Linux, macOS, Windows, or MSRV jobs through this plan.

PR refs and branch refs remain naturally separated by `github.ref`.

### D. Make source synchronization one-way

Set the current action inputs explicitly:

~~~yaml
sync: rsync
copyback: false
~~~

Gregg only needs the checked-out source copied into the FreeBSD VM. It does not consume guest build artifacts after the native test.

The old successful action path copies the entire guest work tree, including a large `target/debug` tree, back to the Ubuntu host during teardown. Recent logs show more than 100 MiB transferred back despite no later step using those bytes.

Disabling copyback:

- removes unnecessary CI I/O;
- reduces post-test runtime;
- reduces the amount of VM synchronization work exposed to transport stalls;
- does not alter the test or its evidence.

Keep `sync: rsync` explicit so a future upstream default change does not silently alter Gregg's CI transport.

Do not enable `cache-after-prepare` in this pass. It adds another shutdown/boot transition and is an optimization, not necessary to correct the reliability defect.

### E. Preserve failure visibility

The final FreeBSD job must remain required in the same sense it is today:

- no `continue-on-error`;
- no fail-open `if:` around the VM action;
- no automatic conversion to compile-only cross-target checking;
- no silent retry loop that can multiply the timeout budget;
- no scheduled-only downgrade.

If the upgraded action cannot boot FreeBSD 14.2 within the repository-owned bound, the job should fail and expose the action/bootstrap logs.

A manual rerun remains an operator option for transient hosted-infrastructure failures; it is not part of the workflow implementation.

### F. Reconcile live CI documentation

Inspect current documentation for statements that specifically name the old FreeBSD VM action/version or imply an unbounded FreeBSD job.

At minimum inspect:

- `AGENTS.md`;
- `plans/README.md`;
- `architecture/collectors.md`;
- `.opencode/skills/platform-collectors/SKILL.md` if it documents CI mechanics.

Only update live documentation that is stale because of this workflow correction. Do not rewrite completed Plans 136-137 to claim they used the new action; their historical evidence remains valid.

No user-facing `CHANGELOG.md` entry is required unless implementation changes supported FreeBSD behavior, which this plan explicitly does not intend.

## Verification

Before pushing the implementation, inspect the resulting workflow diff and confirm the FreeBSD job contains exactly one VM action invocation with:

- `vmactions/freebsd-vm@v1.5.8`;
- `release: "14.2"`;
- `sync: rsync`;
- `copyback: false`;
- no `usesh`;
- a twenty-minute job timeout;
- FreeBSD-only same-ref concurrency cancellation.

Run the ordinary local repository checks appropriate for a workflow-only change:

~~~text
cargo fmt --all -- --check
./scripts/check-local.sh
~~~

The implementation commit must then run the existing CI workflow. Closure requires:

- Linux green;
- macOS arm64 green;
- macOS Intel green;
- Windows green including SCM smoke;
- MSRV Rust 1.89 green;
- FreeBSD 14.2 native `gregg-host` qualification green under the upgraded VM action.

One ordinary green implementation run is sufficient success-path evidence; this plan does not require repeated-green ceremony.

The bounded-failure property is established structurally by the committed `timeout-minutes` value. A deliberate six-hour reproduction or forced hung VM is not required.

Record the exact implementation SHA and exact final CI run ID in the closure record.

If the FreeBSD job fails before Gregg tests under the new action, diagnose whether the failure is action configuration, upstream image availability, or VM bootstrap. Do not weaken native tests or change collector semantics to make CI green.

## Acceptance criteria

- [ ] `.github/workflows/ci.yml` no longer uses `vmactions/freebsd-vm@v1.1.9`.
- [ ] The FreeBSD job uses current `vmactions/freebsd-vm@v1.5.8`.
- [ ] FreeBSD `release: "14.2"` remains pinned exactly.
- [ ] The deprecated/ignored `usesh` input is removed.
- [ ] The FreeBSD job has a repository-owned `timeout-minutes: 20` outer bound.
- [ ] A VM bootstrap/readiness wedge can no longer hold the job until GitHub's six-hour limit.
- [ ] Same-ref superseded FreeBSD jobs are cancelled without applying concurrency cancellation to the other CI jobs.
- [ ] Source synchronization remains explicit `rsync`.
- [ ] `copyback: false` prevents the unused guest build tree from being copied back to the Ubuntu runner.
- [ ] `cache-after-prepare` is not introduced as part of this reliability correction.
- [ ] The FreeBSD job remains blocking/fail-closed; no `continue-on-error`, skip, or compile-only downgrade is introduced.
- [ ] The native test command remains `cargo test -p gregg-host --all-features`.
- [ ] No FreeBSD collector production code, protocol behavior, daemon behavior, metric semantics, or support claim changes.
- [ ] No Linux/macOS/Windows/MSRV CI behavior changes.
- [ ] Live CI documentation no longer names the obsolete FreeBSD action/version where applicable; Plans 136-137 remain historical.
- [ ] One ordinary implementation CI run is green across all six current jobs.
- [ ] Closure records the exact implementation SHA and CI run ID.

## Explicit non-goals

Do not include:

- changing FreeBSD collector formulas or FFI layouts;
- changing the Plan-137 native loopback or disk-write evidence semantics;
- adding FreeBSD swap or CPU-frequency support;
- adding NetBSD/OpenBSD;
- making `greggd` a supported FreeBSD daemon/service target;
- FreeBSD rc.d, installer, package, or release-binary work;
- changing the FreeBSD qualification release from 14.2;
- replacing GitHub-hosted runners with self-hosted runners;
- introducing a dedicated FreeBSD workflow solely for this correction;
- broad CI concurrency changes;
- automatic retries;
- VNC/tmate/debug shells in ordinary CI;
- a custom fork of `vmactions/freebsd-vm` unless the maintained current action demonstrably cannot satisfy the bounded 14.2 job;
- enabling VM prepared-image caching merely for speed;
- upgrading unrelated GitHub Actions such as `actions/checkout`;
- dependency, Rust MSRV, product API, TUI, installer, updater, or release changes.

## Handoff note

Start with `.github/workflows/ci.yml` only.

The key invariant is that the native FreeBSD qualification remains exactly as strong while the infrastructure around it becomes bounded and current. If `vmactions/freebsd-vm@v1.5.8` exposes an incompatibility with Gregg's exact FreeBSD 14.2 job, diagnose that incompatibility before considering any broader workflow redesign. Do not paper over an upstream bootstrap failure by weakening `gregg-host` tests.
