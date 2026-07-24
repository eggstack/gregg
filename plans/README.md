# Gregg version-1 plan index

This directory contains the implementation roadmap and execution-ready plans for taking Gregg through a credible, crates.io-published version 1.

The plans are ordered by dependency. A later phase may begin early only where its interfaces are already frozen and doing so does not bypass an earlier acceptance gate.

`1.0.0` was published after the initial implementation attempt. Phases 11 through 13 corrected most known source-level defects, including Darwin ABI and Mach ownership, daemon pre-bind and supervision, endpoint persistence and locking, transactional editing, polling cadence, response bounds, installer paths, non-root systemd identity, launchd target construction, and bounded native tests. The latest review found two narrow source details plus release-workflow, native-platform, lifecycle, measurement, soak, evidence, tag, and publication gaps. Phase 14 is the active source of truth for final `1.0.1` execution and closure.

| Plan | Purpose | Primary output | Status |
| --- | --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) | Program-level architecture, sequencing, risks, and release definition | Version-1 execution map | active |
| [`001-foundation-workspace-protocol.md`](001-foundation-workspace-protocol.md) | Workspace, package metadata, protocol schema, fixtures, CI foundation | Publishable protocol crate and stable contracts | implemented |
| [`002-linux-metrics-collector.md`](002-linux-metrics-collector.md) | Native Linux identity and metric sampling | Tested Linux collector | implementation landed; final frozen-candidate x86-64 and ARM64 evidence pending phase 14 |
| [`003-macos-metrics-collector.md`](003-macos-metrics-collector.md) | Native Darwin/Mach/sysctl metric sampling | Tested macOS collector | implementation landed; final frozen-candidate Intel, Apple Silicon, and leak evidence pending phase 14 |
| [`004-daemon-sampler-http-api.md`](004-daemon-sampler-http-api.md) | Cached sampler, readiness, HTTP API, shutdown | Functional foreground daemon | source implementation landed; installed-binary, lifecycle, resource, and soak evidence pending phase 14 |
| [`005-daemon-config-service-packaging.md`](005-daemon-config-service-packaging.md) | Atomic config, lifecycle CLI, systemd, launchd, installation | Deployable `greggd` | partially implemented; launchd stop idempotence and native systemd/launchd closure pending phase 14 |
| [`006-client-config-cli.md`](006-client-config-cli.md) | Endpoint model and `add/list/remove/refresh/edit` commands | Scriptable client configuration | partially implemented; secure final-file permissions and installed-client evidence pending phase 14 |
| [`007-polling-state-engine.md`](007-polling-state-engine.md) | Bounded polling, batch generations, state reduction, ordering | Maintainable non-visual client core | implementation landed; frozen-candidate mixed-fleet soak evidence pending phase 14 |
| [`008-compact-ratatui-tui.md`](008-compact-ratatui-tui.md) | Four-line rendering, adaptive width, navigation, scrolling | Complete compact TUI | implementation landed; final native/manual and long-run evidence pending phase 14 |
| [`009-testing-hardening-performance.md`](009-testing-hardening-performance.md) | Cross-platform failures, soak tests, resource budgets, packaging validation | Release-candidate evidence | partially implemented; package, native, lifecycle, measurement, and soak gates pending phase 14 |
| [`010-cratesio-release-v1.md`](010-cratesio-release-v1.md) | Documentation closure, package verification, publication and tagging | crates.io version 1.0.0 release | `1.0.0` published; corrective `1.0.1` release and original evidence gaps pending phase 14 |
| [`011-v1.0.1-corrective-closure.md`](011-v1.0.1-corrective-closure.md) | First correction of native, runtime, client, CI, and documentation defects | Initial `1.0.1` corrective implementation | partially implemented; unresolved closure superseded by later phases |
| [`012-v1.0.1-final-corrective-closure.md`](012-v1.0.1-final-corrective-closure.md) | Correct remaining native, concurrency, scheduler, lifecycle, packaging, evidence, and release defects | Broad `1.0.1` corrective implementation | partially implemented; remaining closure superseded by phases 13 and 14 |
| [`013-v1.0.1-release-gate-closure.md`](013-v1.0.1-release-gate-closure.md) | Close launchd, edit transaction, failure cleanup, package workflow, native evidence, soak, and publication gaps | Corrected source and initial release-candidate workflow | partially implemented; source corrections largely landed, final execution superseded by phase 14 |
| [`014-v1.0.1-final-release-execution.md`](014-v1.0.1-final-release-execution.md) | Close final source details, repair staged release evidence, execute native/lifecycle/soak gates, and publish `1.0.1` | Verified tagged and published `1.0.1` release | active |

## Completion rule

A plan is not complete merely because its implementation has landed. It is complete only when all acceptance criteria are demonstrated by tests, CI, reproducible commands, or documented manual evidence appropriate to the target platform.

A plan may be marked `implemented` only after every acceptance criterion is satisfied with evidence in the tree or an immutable linked CI/release record. Mock coverage, source inspection, version bumps, reduced green CI, cross-compilation, console-only checksums, or compensating reasoning cannot substitute for required native Linux/macOS, service-manager, package, measurement, soak, tag, or publication evidence.

Phase 14 may be marked implemented only after one frozen source SHA passes all package, stable/MSRV, native-platform, systemd, launchd, resource, soak, dependency, and clean-install gates; annotated `v1.0.1` peels to that SHA; all three `1.0.1` crates are published and registry-verified; and the complete immutable evidence is reconciled in `plans/v1.0.1-final-evidence.md`.

Any discovered scope expansion should be recorded as a post-version-1 idea unless it is necessary for correctness, safety, publishability, or the explicit product contract in `README.md`.
