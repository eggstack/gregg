# Gregg version-1 plan index

This directory contains the implementation roadmap and execution-ready plans for taking Gregg through a credible, crates.io-published version 1.

The plans are ordered by dependency. A later phase may begin early only where its interfaces are already frozen and doing so does not bypass an earlier acceptance gate.

`1.0.0` was published after the initial implementation attempt. Phases 11 and 12 corrected most known source-level defects, including the Darwin ABI and Mach ownership path, daemon pre-bind and stale-snapshot behavior, endpoint port persistence, cross-process mutation locking, polling cadence, response bounds, installer paths, and non-root systemd identity. Review after the Phase 12 implementation found a smaller remaining set of launchd, transactional editing, failure-cleanup, bounded-test, package-verification, native-platform, lifecycle, soak, evidence, and publication gaps. Phase 13 is the active source of truth for final `1.0.1` release closure.

| Plan | Purpose | Primary output | Status |
| --- | --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) | Program-level architecture, sequencing, risks, and release definition | Version-1 execution map | active |
| [`001-foundation-workspace-protocol.md`](001-foundation-workspace-protocol.md) | Workspace, package metadata, protocol schema, fixtures, CI foundation | Publishable protocol crate and stable contracts | implemented |
| [`002-linux-metrics-collector.md`](002-linux-metrics-collector.md) | Native Linux identity and metric sampling | Tested Linux collector | implementation landed; final x86-64/ARM64 release evidence pending phase 13 |
| [`003-macos-metrics-collector.md`](003-macos-metrics-collector.md) | Native Darwin/Mach/sysctl metric sampling | Tested macOS collector | implementation landed; final Intel/Apple Silicon and resource evidence pending phase 13 |
| [`004-daemon-sampler-http-api.md`](004-daemon-sampler-http-api.md) | Cached sampler, readiness, HTTP API, shutdown | Functional foreground daemon | partially implemented; abnormal-exit cleanup closure in phase 13 |
| [`005-daemon-config-service-packaging.md`](005-daemon-config-service-packaging.md) | Atomic config, lifecycle CLI, systemd, launchd, installation | Deployable `greggd` | partially implemented; launchd and native lifecycle closure in phase 13 |
| [`006-client-config-cli.md`](006-client-config-cli.md) | Endpoint model and `add/list/remove/refresh/edit` commands | Scriptable client configuration | partially implemented; transactional edit closure in phase 13 |
| [`007-polling-state-engine.md`](007-polling-state-engine.md) | Bounded polling, batch generations, state reduction, ordering | Maintainable non-visual client core | implementation landed; final soak evidence pending phase 13 |
| [`008-compact-ratatui-tui.md`](008-compact-ratatui-tui.md) | Four-line rendering, adaptive width, navigation, scrolling | Complete compact TUI | implementation landed; final native/manual and soak evidence pending phase 13 |
| [`009-testing-hardening-performance.md`](009-testing-hardening-performance.md) | Cross-platform failures, soak tests, resource budgets, packaging validation | Release-candidate evidence | partially implemented; package, native, measurement, and soak gates pending phase 13 |
| [`010-cratesio-release-v1.md`](010-cratesio-release-v1.md) | Documentation closure, package verification, publication and tagging | crates.io version 1.0.0 release | `1.0.0` published; original closure criteria incomplete |
| [`011-v1.0.1-corrective-closure.md`](011-v1.0.1-corrective-closure.md) | First correction of native, runtime, client, CI, and documentation defects | Initial `1.0.1` corrective implementation | partially implemented; unresolved items superseded by later phases |
| [`012-v1.0.1-final-corrective-closure.md`](012-v1.0.1-final-corrective-closure.md) | Correct remaining native, concurrency, scheduler, lifecycle, packaging, evidence, and release defects | Broad `1.0.1` corrective implementation | partially implemented; final release gates superseded by phase 13 |
| [`013-v1.0.1-release-gate-closure.md`](013-v1.0.1-release-gate-closure.md) | Close launchd, edit transaction, failure cleanup, staged packaging, native evidence, soak, and publication gaps | Verified and published `1.0.1` release | active |

## Completion rule

A plan is not complete merely because its implementation has landed. It is complete only when all acceptance criteria are demonstrated by tests, CI, reproducible commands, or documented manual evidence appropriate to the target platform.

A plan may be marked `implemented` only after every acceptance criterion is satisfied with evidence in the tree or an immutable linked CI/release record. Mock coverage, source inspection, version bumps, reduced green CI, and compensating reasoning cannot substitute for required native Linux/macOS, service-manager, package, measurement, soak, tag, or publication evidence.

Phase 13 may be marked implemented only after the published `1.0.1` source, annotated tag, package checksums, clean archive and registry installs, native platform runs, systemd and launchd lifecycle results, dependency disposition, and measured resource/soak evidence are reconciled in `plans/v1.0.1-final-evidence.md`.

Any discovered scope expansion should be recorded as a post-version-1 idea unless it is necessary for correctness, safety, publishability, or the explicit product contract in `README.md`.