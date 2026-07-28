# Release evidence architecture

Release evidence has three boundaries. Current-run dispatch output is a
diagnostic summary and may be created before GitHub assigns artifact IDs.
Cross-run pre-tag and final manifests are immutable selections: each logical
stage binds to one retrieved GitHub artifact and records its numeric ID, exact
name, downloaded ZIP SHA-256, and size.

The finalizer checks out the frozen candidate SHA for executable tooling. Run
selection and the operator-authorized historical disposition created after that
freeze are data supplied through bounded base64 workflow inputs. Each decoded
UTF-8 JSON document is validated before network access, written atomically,
hashed, and included with workflow run and actor identity. The selection source
remains `workflow-dispatch-base64` through retrieval and aggregation.

Package provenance declares exact relative archive and lockfile paths. The
retrieval index resolves only those declarations, verifies containment, digest,
size, package name, and version, and rejects ambiguity. Boundary 2 reuses the
selected Boundary-1 archive bytes, semantically compares the Cargo.lock
protocol checksum with the validated registry response, and retains locked
build, test, install, help/version, and tool-version command transcripts.

Boundary-2 success requires non-null equality among the registry record,
generated lockfile, and selected protocol archive checksum. Production accepts
only the approved crates.io sources. Nonpublishing qualification uses a named
alternate sparse registry bound to loopback; directory replacement is
insufficient because Cargo preserves the replaced crates.io identity and may
omit the registry checksum. Qualification-only registry flags are forbidden in
production workflows.

Each dependent package produces `command-evidence-index.json`. The index binds
the exact archive, registry record, generated lockfile, normalized manifest,
installed binary, and every required command record/stdout/stderr file to its
digest and size. It is independently validated before the Boundary-2 summary is
written. Candidate artifact declarations are then derived from that validated
index and rechecked for containment, existence, digest, and size before upload.

Full-contract qualification loads
`plans/evidence/release-requirements.json`,
`plans/evidence/release-dispatch-contract.json`, and
`plans/evidence/phase34-qualification-contract.json`. Synthetic platform
records prove orchestration only, but the pre-tag and final manifests must
contain the exact production stage sets and bind each stage to one immutable
artifact identity.

Final singleton evidence follows this order:

```text
selection decode -> artifact retrieval -> role index
-> singleton materialization -> final aggregation -> manifest validation
```

The canonical singleton roles are `registry-summary` and
`version-1.0.0-disposition`. Their role records retain workflow run, attempt,
artifact ID/name, ZIP identity, extracted path, and file identity. Aggregation
consumes the materialized copies, never an equivalent direct path.

The sustained workload retains the scheduler task handle. Cancellation is
clean only after a bounded join returns success; channel closure alone cannot
qualify shutdown, and timeout cleanup aborts and awaits the task.
