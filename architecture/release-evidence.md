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

## Evidence lineage (Phase 35)

Each retrieved-manifest stage binding is a canonical record carrying:

- `stage`, `workflow_run_id`, `workflow_run_attempt`;
- `artifact` with `id`, `name`, `zip_sha256`, `zip_size_bytes`, and a
  normalized `extraction_root` contained under the evidence directory;
- `candidate` with the exact `path`, `sha256`, and `size_bytes` of the
  `candidate.json` found inside the extracted artifact.

Stage bindings are derived by retrieval, never synthesized after retrieval.
Duplicate, missing, ambiguous, or conflicting stage bindings fail. Conflicting
artifact-ID reuse fails. Candidate/run/attempt mismatch fails.

Package archive lineage is recorded for all three crates. For `gregg-protocol`,
the chain is:

```text
protocol-prepublish archive -> local sparse-registry archive
-> protocol-index checksum -> final package provenance
```

For `greggd` and `gregg`, the chain is:

```text
binary-prepublish archive -> Boundary-2 input archive
-> Boundary-2 archive-before identity -> Boundary-2 archive-after identity
-> final package provenance
```

Boundary-1 archives are built exactly once from valid crate trees and reused
unchanged through Boundary 2, final package provenance, and publication
authorization. Any one-byte mutation, repack, or package swap at any boundary
fails.

Singleton-file lineage records retain stage, run/attempt, artifact ID/name,
ZIP digest/size, extraction root, candidate path/identity, declared
candidate artifact path/role/digest/size, extracted file path/digest/size, and
materialized path/digest/size. The candidate declaration, extracted file, and
materialized file must have identical digest and size.

## Package provenance

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

## Boundary-2 artifact topology

Each Boundary-2 execution produces a real stage artifact ZIP containing:

```text
candidate.json
registry-reverify-<package>.json
protocol-registry-record.json
command-evidence/command-evidence-index.json
command-evidence/archive-identity.json
command-evidence/lockfile-identity.json
command-evidence/normalized-manifest.json
command-evidence/*.json
command-evidence/*.stdout
command-evidence/*.stderr
artifacts.json
```

The candidate is generated through the production `write-candidate` path with
`--artifact-root` validation. The ZIP is registered with the mock GitHub API
using one run ID, attempt, numeric artifact ID, and exact artifact name. The
final selection references these exact bindings; generic replacement candidates
are rejected.

## Postpublish evidence (Phase 35)

One authoritative production stage creates all postpublish source-of-record
evidence: `registry-summary.json`, `1.0.0-disposition.json`,
`disposition-decision.json`, `disposition-decision-identity.json`, installed
verification evidence, `candidate.json`, and `artifacts.json`. The production
finalizer must not independently recreate these files.

The synthetic postpublish ZIP physically contains every file declared by its
candidate. Role records are derived from the retrieved candidate declarations:
for each role, the declared relative path is resolved inside the exact
extracted artifact root, the candidate-declared digest and size are verified,
and duplicate, missing, wrong-stage, or undeclared roles are rejected.

The operator decision identity is bound exactly once and is traceable. The
preferred contract lets the postpublish producer receive the operator decision
input, record its identity, and the finalizer rely on the selected postpublish
artifact.

## Final input materialization

The shared helper `scripts/prepare-final-release-inputs.py` is invoked by both
the production finalizer and the qualification harness. It validates
selection/retrieved-manifest identity, locates the selected postpublish
artifact, reads and validates its candidate, derives singleton role records,
verifies extracted file path/digest/size/containment, materializes each
singleton, verifies copied identity, and writes a role index with relative
materialized paths.

Final aggregation consumes only role-indexed materialized paths. Direct
registry/disposition paths fail in production mode. Missing role index fails.
Role paths outside the selected extraction root fail. Files changed after role
indexing or before aggregation fail.

## Full-contract qualification

Full-contract qualification loads
`plans/evidence/release-requirements.json`,
`plans/evidence/release-dispatch-contract.json`, and
`plans/evidence/phase35-qualification-contract.json`. Synthetic platform
records prove orchestration only, but the pre-tag and final manifests must
contain the exact production stage sets and bind each stage to one immutable
artifact identity.

The qualification upload explicitly retains hidden files because the retrieval
tool uses bounded `.retrieval-downloads-*` directories. Omitting those
directories makes the summary's complete file index unreplayable and causes
independent downloaded-artifact validation to fail.

Final singleton evidence follows this order:

```text
selection decode -> artifact retrieval -> role index
-> singleton materialization -> final aggregation -> manifest validation
```

The canonical singleton roles are `registry-summary` and
`version-1.0.0-disposition`. Their role records retain workflow run, attempt,
artifact ID/name, ZIP identity, extracted path, and file identity. Aggregation
consumes the materialized copies, never an equivalent direct path. Materialized
paths are role-index-relative so downloaded evidence remains replayable; the
independent validator reopens each copy and checks its digest and size.

## Independent validation

The independent validator (`validate-qualification-output.py`) invokes
production `validate-manifest` for candidate and final manifests, then performs
additional cross-binding checks: Boundary-2-to-final binding, archive
continuity, postpublish artifact membership, contract identity, hosted
identity, and execution order. Self-consistent but cross-unbound evidence fails.

## Sustained workload

The sustained workload retains the scheduler task handle. Cancellation is
clean only after a bounded join returns success; channel closure alone cannot
qualify shutdown, and timeout cleanup aborts and awaits the task.
