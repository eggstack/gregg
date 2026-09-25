# Plan 129: macOS route-message parser corrective pass

Status: planned.

Depends on: completed Plan 128 and the current post-Plan-128 macOS collector baseline. This work is independent of the remaining Plan 091 soak record.

## Objective

Correct one post-Plan-128 macOS network-collector defect: the `NET_RT_IFLIST2` parser currently requires every route message to be at least `size_of::<libc::if_msghdr2>()` before it inspects the message type. Darwin's `NET_RT_IFLIST2` stream is heterogeneous: an `RTM_IFINFO2` record is followed by variable-length address and multicast records such as `RTM_NEWADDR` and `RTM_NEWMADDR2` before later interfaces are emitted.

A valid shorter non-`RTM_IFINFO2` record can therefore terminate Gregg's parser early and hide later interfaces. The existing native v2 smoke only requires `payload.network.is_some()`, so a partial interface list can pass CI even if the traffic-bearing interface was never reached.

This is a narrow parser/test/record-correction phase. Do not reopen the Plan-128 filesystem ABI fix, the typed `getifaddrs` fallback, the IOKit disk-I/O source, protocol schema, client normalization, TUI behavior, sampling cadence, or readiness policy.

## Post-Plan-128 finding

Plan 128 correctly replaced the unsafe `getifaddrs -> if_data64` cast and moved the preferred source to `NET_RT_IFLIST2` / `if_msghdr2` / `if_data64`.

The remaining parser has this effective ordering:

~~~text
read msglen
if msglen < size_of::<if_msghdr2>() => stop
if message type == RTM_IFINFO2 => parse
else => skip
~~~

That assumes every message in the buffer uses the `if_msghdr2` layout.

Darwin's own `sysctl_iflist2()` implementation does not make that guarantee. It emits:

~~~text
RTM_IFINFO2
RTM_NEWADDR...
RTM_NEWMADDR2...
RTM_IFINFO2
...
~~~

where the non-interface records are produced with their own message layouts and lengths.

The current deterministic regression `iflist2_parser_skips_unrelated_messages` does not expose the defect because its synthetic unrelated `RTM_NEWADDR` is encoded inside a full `libc::if_msghdr2` allocation and advertises `size_of::<if_msghdr2>()` as its message length.

References for implementation review:

- Darwin XNU `bsd/net/rtsock.c::sysctl_iflist2`;
- Darwin routing-message headers in `bsd/net/route.h` / libc bindings;
- Plan 128's current parser and native-v2 smoke.

## Required implementation

### 1. Parse the common route-message prefix before type-specific layouts

Refactor `parse_iflist2_buffer()` so structural validation is performed in two stages.

For every message:

1. require only enough bytes to read the common route-message prefix that contains at least `msglen`, version, and type;
2. read `msglen` without assuming `if_msghdr2` alignment/layout;
3. reject `msglen == 0`;
4. reject `msglen` shorter than the selected common-prefix minimum;
5. reject `offset + msglen > buffer.len()`;
6. inspect the message type;
7. if type is `RTM_IFINFO2`, require `msglen >= size_of::<libc::if_msghdr2>()` before reading the full header;
8. if type is unrelated, advance by exactly `msglen` and continue;
9. never stop merely because an unrelated valid message is shorter than `if_msghdr2`.

Use libc/Darwin definitions where available. Do not introduce a private full routing-message ABI merely to read the small common prefix.

If libc does not expose a suitable common header type, a minimal prefix reader over the first bytes is acceptable when it is explicitly documented and tested against Darwin's field widths/order. Keep that helper local to the parser.

### 2. Preserve malformed-tail safety

The parser must still fail closed on malformed input:

- zero `msglen`;
- `msglen` shorter than the common prefix;
- `msglen` larger than the remaining buffer;
- truncated `RTM_IFINFO2`;
- arithmetic overflow in offset advancement;
- impossible message-count growth.

The parser may return records parsed before a malformed tail, matching the current bounded/truncating behavior, but it must never read out of bounds or loop forever.

Do not turn malformed input into a source-wide panic.

### 3. Make deterministic fixtures structurally realistic

Replace or augment the current synthetic unrelated-message helper.

At minimum add:

- a realistic short `RTM_NEWADDR` message between two valid `RTM_IFINFO2` records;
- a second unrelated route-message type where practical, preferably `RTM_NEWMADDR2` or another message actually emitted by `sysctl_iflist2()`;
- a short unrelated message immediately after the first interface;
- multiple unrelated messages between two interfaces;
- a malformed short message that must terminate safely;
- a truncated `RTM_IFINFO2` that must not be read as a full header.

The key regression must prove that two interface records survive when valid shorter unrelated records are interleaved.

Do not keep a misleading `RTM_NEWADDR` fixture whose advertised length is `size_of::<if_msghdr2>()` unless the test is explicitly about oversized unrelated messages.

### 4. Strengthen native network verification

The Plan-128 native v2 smoke currently treats `payload.network.is_some()` as sufficient.

Strengthen the native macOS proof so it detects a partial list that contains only loopback or otherwise misses ordinary interfaces.

At minimum, on the existing GitHub-hosted macOS images:

- require the raw preferred/native enumeration to contain at least one non-loopback interface;
- require nonempty stable ids/names for returned interfaces;
- require the complete v2 payload to contain at least one non-loopback interface after bounded warmup;
- continue to allow zero byte rates during an idle interval;
- continue to allow unknown link capacity;
- do not require a specific interface name such as `en0` or `en1`.

If hosted-runner topology proves too unstable for a non-loopback assertion, use the strongest architecture-neutral invariant supported by both current native runners and document why. Do not silently fall back to `network.is_some()` without proving it can catch the original partial-parser defect.

### 5. Keep fallback semantics unchanged

Do not broaden this pass into fallback redesign.

The Plan-128 `getifaddrs` fallback remains:

- `AF_LINK ifa_data -> libc::if_data`;
- 32-bit counters widened to `u64`;
- counter decrease/wrap handled by shared re-baselining;
- unknown/unrepresentable capacity stays `None`;
- deterministic sort/dedup by interface identity.

Only touch fallback code if a parser-focused test exposes a direct regression introduced by the corrective change.

### 6. Reconcile the Plan-128 record truthfully

Plan 128 is a closed historical implementation record. Do not rewrite its implementation SHA, CI evidence, or original closure narrative.

Append a correction note that states:

- post-closure review found the heterogeneous-route-message parsing defect;
- the original statement that all acceptance boxes held was too broad for the `NET_RT_IFLIST2` length-checked parser criterion;
- the plan file also retained unchecked acceptance boxes despite the closure narrative;
- Plan 129 owns the parser correction and final record reconciliation;
- the filesystem ABI correction, typed `if_data` fallback, diagnostics, and native Intel/arm64 CI expansion remain valid and are not reopened.

Do not retroactively mark every Plan-128 box checked. Preserve history and let Plan 129 close the newly discovered defect.

When Plan 129 closes, update `plans/README.md` to show Plan 128 as complete with a Plan-129 corrective follow-up and Plan 129 as complete with its implementation/CI evidence.

## Verification

Run deterministic parser tests first:

~~~text
cargo test -p greggd --all-features -- collector::macos::ffi
cargo test -p greggd --all-features -- collector::macos
~~~

Then run the ordinary local gates:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Use the existing ordinary CI workflow at the final implementation SHA.

Both native macOS jobs must pass:

- `macos-15`;
- `macos-15-intel`.

The existing macOS collector job is the required native proof. Do not add a new workflow, self-hosted runner, or dedicated qualification matrix.

Linux, Windows, and MSRV Rust 1.89 must remain green because the ordinary workflow covers the full repository and shared code must not regress.

## Acceptance criteria

- [ ] The route-message parser no longer requires unrelated messages to be `if_msghdr2`-sized before type discrimination.
- [ ] A bounded common-prefix validation step safely obtains `msglen` and message type before any type-specific cast/read.
- [ ] `RTM_IFINFO2` alone requires `size_of::<libc::if_msghdr2>()` before the full header is read.
- [ ] Valid shorter unrelated messages advance by their own `msglen` and do not terminate parsing.
- [ ] Zero-length, undersized-common-prefix, oversized/truncated, and truncated-`RTM_IFINFO2` cases remain bounded and out-of-bounds safe.
- [ ] Deterministic tests interleave a realistically short `RTM_NEWADDR` between two `RTM_IFINFO2` records and prove both interfaces are returned.
- [ ] Deterministic tests cover at least one additional unrelated routing-message shape or repeated unrelated records between interfaces.
- [ ] The misleading full-`if_msghdr2` unrelated-message fixture is removed, renamed, or supplemented so it cannot mask this defect.
- [ ] Native macOS verification proves at least one non-loopback interface reaches the raw/native result and complete v2 payload, or records an equally strong architecture-neutral invariant if hosted topology requires it.
- [ ] Both existing macOS arm64 and Intel CI jobs pass the corrected parser/native-v2 suite.
- [ ] Plan-128 filesystem-capacity ABI handling, typed `if_data` fallback, IOKit disk-I/O behavior, diagnostics, protocol, TUI, cadence, and readiness semantics are unchanged.
- [ ] No new dependency, external metrics command, privilege requirement, protocol field, or workflow is introduced.
- [ ] Plan 128 receives an appended correction note rather than rewritten historical closure evidence.
- [ ] Plan 129 closure records the implementation SHA and exact existing CI run used.

## Explicit non-goals

Do not include:

- another macOS network data source;
- interface-name heuristics;
- per-flow or per-process network accounting;
- packet capture;
- Wi-Fi PHY/signal metadata;
- NetworkExtension/SystemConfiguration adoption;
- shelling out to `netstat`, `ifconfig`, `networksetup`, or other tools;
- drive/statfs changes;
- IOKit storage changes;
- protocol or TUI changes;
- sampler cadence changes;
- general route-socket parsing infrastructure;
- a reusable routing-message crate;
- new CI infrastructure.

## Handoff note

Start in `crates/greggd/src/collector/macos/ffi.rs`.

The important boundary is simple: route-message framing is common, but message bodies are heterogeneous. Read and validate the framing first, then apply `if_msghdr2` size/layout requirements only to `RTM_IFINFO2`.

Do not treat current green native CI as proof that all interfaces were parsed. The Plan-128 smoke proved that at least one network payload could materialize; Plan 129 must prove that valid interleaved Darwin route messages do not prevent later interfaces from reaching the collector.
