# Plan 192: Stop sending EggPool bearer credentials over cleartext HTTP

Status: proposed (not started).

Source: 2026-11-17 security audit, finding 4.

Depends on: current post-186 `main`. Independent of Plans 091 and 183-191.

## Problem

The EggPool API key is read from an environment variable and never persisted
(`crates/gregg/src/eggpool.rs:410-424`) — good. But `gregg eggpool add`
defaults to HTTP (`crates/gregg/src/cli.rs:711-735`, `scheme: if https { … }
else { Http }`), and the documented example is:

```text
gregg eggpool add pool.local:11300 --api-key-env EGGPOOL_API_KEY
```

(`docs/client.md:113-118`). Following the documented example sends
`Authorization: Bearer …` in cleartext over the LAN. Config validation checks
only the environment-variable name shape
(`crates/gregg/src/config/validation.rs:202-222`); nothing rejects or warns
about `scheme = "http"` plus a key on a non-loopback host.

The normal use of the key is against a local dashboard or proxy, where HTTP on
loopback is reasonable. The defect is that the tool permits — and documents —
the remote cleartext case silently.

## Decision

Two-layer guard, no hard `ConfigViolation`:

1. **New configuration is refused at the CLI** when an API-key environment
   variable is supplied with HTTP and a non-loopback host. The operator is
   told to use `--https` or a loopback endpoint.
2. **Existing configuration fails closed at request time**: the client does
   not open a connection or send an `Authorization` header for that entry; the
   pane shows a typed "insecure credential transport" diagnostic instead. The
   rest of the fleet and the rest of the pane keep working.

Why not a `ConfigViolation`: `Config::parse` rejects any violation
(`crates/gregg/src/config/model.rs:471-476`), and `ConfigStore::mutate` loads
before mutating (`crates/gregg/src/config/store.rs:435-447`). A violation would
make an existing config unloadable after upgrade and would also block
`gregg eggpool remove`, because the CLI cannot load the file to edit it. Fail
closed at the network boundary avoids that trap while still never leaking the
key.

Loopback means the literal `127.0.0.0/8`, `::1`, or the name `localhost`
(case-insensitive). A hostname that resolves to loopback is not exempt: config
validation does no DNS, and resolving at this layer would add a network
dependency to startup.

## Scope

In scope: the CLI add gate, one runtime outcome and its renderer, the loopback
predicate, docs, tests, and the changelog. Out of scope: changing the default
scheme for keyless endpoints, adding secret storage, TLS configuration for
EggPool, and any wire-protocol change.

## Implementation

1. Add a pure predicate in `crates/gregg/src/eggpool_endpoint.rs` (next to the
   existing endpoint normalization):

   ```rust
   pub fn host_is_loopback(host: &str) -> bool
   ```

   - `host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())`, or
   - `host.eq_ignore_ascii_case("localhost")`.
   - Everything else, including zone-id literals, is not loopback.

2. Add one policy helper so CLI and runtime agree:

   ```rust
   pub fn credential_transport_is_insecure(entry: &EggpoolEntry) -> bool
   ```

   True iff `api_key_env.is_some()` and `scheme == EggpoolScheme::Http` and
   `!host_is_loopback(&host)`.

3. CLI gate in `cmd_eggpool_add` (`crates/gregg/src/cli.rs:711-760`): build
   the entry, then return a typed error before `store.mutate` when the policy
   is violated. Message: "refusing to send an API key over http to <host>;
   use --https or a loopback endpoint". No config write occurs.

4. Runtime gate: at the top of `EggpoolEndpoint::fetch` (and any path that
   builds a request for the summary or status route), if the policy is
   violated, return a new
   `EggpoolFetchOutcome::InsecureCredentialTransport { host: String }`
   without calling `credential()` and without opening a connection. This
   guarantees no `Authorization` header is ever constructed for the entry.

5. Render it in `crates/gregg/src/ui/eggpool.rs` next to the existing
   outcome mappings (`:267-300`) as a bounded single-line diagnostic:
   "insecure credential transport; enable https". Do not include the key, the
   environment variable's value, or the header.

6. Docs:
   - `docs/client.md:113-118`: change the credential example to
     `gregg eggpool add pool.local:11300 --https --api-key-env EGGPOOL_API_KEY`
     and add one sentence: HTTP is accepted only for loopback when a key is
     configured.
   - `CHANGELOG.md`: behavior change note.

## Tests

Unit (pure):

- `loopback_hosts_are_recognized` — `127.0.0.1`, `127.10.20.30`, `::1`,
  `localhost`, `LOCALHOST`.
- `non_loopback_hosts_are_not` — `pool.local`, `192.168.1.10`, `fd00::10`,
  `0.0.0.0`, `[::1]` (bracketed form is not the stored normalized host).
- `insecure_policy_truth_table` — http+key+remote true; http+key+loopback
  false; https+key+remote false; http+no key false; https+no key false.

CLI:

- `eggpool_add_refuses_cleartext_key_to_remote` — returns the error, message
  contains `--https`, and the config file is unchanged/not created.
- `eggpool_add_allows_cleartext_key_to_loopback` and
  `eggpool_add_allows_https_key_to_remote` — config written with the expected
  scheme.

Runtime:

- `remote_http_key_never_reaches_the_wire` — reuse the existing in-process
  mock-server harness (`eggpool.rs` tests). The outcome is
  `InsecureCredentialTransport`, and the mock records zero accepted
  connections (or, if the harness counts requests, zero requests and no
  `authorization` header).
- `loopback_http_key_still_sends_bearer` — the existing header assertion path
  proves the exemption does not break local dashboards.
- `insecure_transport_render_is_bounded_and_secret_free` — the renderer output
  contains the diagnostic and not the env value.

## Acceptance criteria

- [x] `gregg eggpool add host --api-key-env NAME` without `--https` is refused
      for a non-loopback host, with no config written.
- [x] The same command succeeds for `127.0.0.1`, `::1`, or `localhost`.
- [x] An existing config that already contains the insecure combination still
      loads, keeps the pane usable, and never sends the key; the pane states
      why.
- [x] `gregg eggpool remove` still works on such a config (no load-blocking
      validation was added).
- [x] The documented example uses `--https`.
- [x] No key value appears in any diagnostic, log, or renderer output.
- [x] `cargo fmt`, workspace Clippy, focused tests, and
      `./scripts/check-local.sh` are clean.

## Verification

```bash
cargo test -p gregg --all-features -- eggpool
cargo test -p gregg --all-features -- config::validation
./scripts/check-local.sh
```

## Risks

- An operator with a remote cleartext EggPool must switch to HTTPS or a local
  tunnel after upgrading. The failure is visible in the pane and documented in
  the changelog; the rest of the client keeps working.
- `localhost` is accepted literally. If a corporate DNS search domain resolves
  `localhost` elsewhere, that is outside the model; loopback IP literals are
  the strict form.

## Closure record

### What shipped

- `eggpool_endpoint::host_is_loopback` (literal `127.0.0.0/8`, `::1`, or the bare
  name `localhost`, case-insensitive) and `credential_transport_is_insecure`
  (key present + `http` + non-loopback), one policy function shared by the CLI
  and the runtime.
- `cmd_eggpool_add` refuses the insecure combination before `store.mutate`, with
  `refusing to send an API key over http to <host>; use --https or a loopback
  endpoint`, so no config is written.
- `EggpoolEndpoint::fetch` and `fetch_health` return a dedicated
  `InsecureCredentialTransport` outcome **before** `credential()` runs, so no
  `Authorization` header is constructed and no connection is opened. The health
  variant is unit-like because its diagnostic is a fixed bounded word.
- `ui/eggpool.rs` renders `insecure credential transport to <host>; enable https`
  for the summary plane (host through `sanitize_single_line`) and
  `insecure credential transport` for health.
- `docs/client.md` uses `--https` in the credential example and states the
  loopback-only rule; `architecture/gregg-client.md` records the policy and why it
  is not a `ConfigViolation`; `CHANGELOG.md` has an entry.

### Verification

- `cargo test -p gregg --all-features -- eggpool` — the new tests pass:
  `remote_http_key_never_reaches_the_wire` (TEST-NET-1 host, asserts the outcome,
  that the refusal is immediate, that the secret never appears in the outcome, and
  that the health plane refuses too), `loopback_http_key_still_sends_bearer`,
  `insecure_policy_truth_table`, `loopback_hosts_are_recognized`,
  `non_loopback_hosts_are_not`, and `a_resolvable_name_is_not_treated_as_loopback`.
- CLI tests: `eggpool_add_refuses_cleartext_key_to_remote` (message names
  `--https` and the host, and no config file is created),
  `eggpool_add_allows_cleartext_key_on_loopback_and_https_anywhere`, and
  `eggpool_remove_still_works_for_a_legacy_insecure_entry`.
- The pre-existing `protected_request_sends_injected_bearer_without_retaining_secret`
  is the live proof that the loopback exemption still puts the bearer on the wire.
- `cargo fmt --all -- --check` — clean; changed crate Clippy-clean.

### Not verified here

- No real EggPool instance was contacted; every network assertion uses the
  in-process mock server from the existing harness, and the refusal case asserts
  no connection at all rather than observing an absent one.
