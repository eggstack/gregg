# gregg-protocol

[![Crates.io](https://img.shields.io/crates/v/gregg-protocol.svg)](https://crates.io/crates/gregg-protocol)
[![Docs.rs](https://docs.rs/gregg-protocol/badge.svg)](https://docs.rs/gregg-protocol)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/gregg-protocol.svg)](https://crates.io/crates/gregg-protocol)

Versioned JSON wire types, metric capabilities, and identity structures shared
by the gregg daemon and client.

## Usage

```rust
use gregg_protocol::StatusSnapshot;

let json = r#"{
  "schema_version": 1,
  "observed_at_unix_ms": 1716460800000,
  "sample_interval_ms": 1000,
  "capabilities": {"cpu_iowait": true},
  "system": {
    "name": "deadpool", "hostname": "deadpool.local",
    "os_name": "linux", "os_version": "Ubuntu 24.04",
    "kernel_name": "Linux", "kernel_release": "6.8.0-31-generic",
    "architecture": "x86_64"
  },
  "cpu": {"logical_cores": 8, "usage_pct": 25.2, "iowait_pct": 0.4},
  "load": {"one": 1.32, "five": 0.91, "fifteen": 0.62},
  "memory": {"used_bytes": 5900000000, "total_bytes": 15600000000, "usage_pct": 37.8},
  "swap": {"used_bytes": 0, "total_bytes": 4000000000, "usage_pct": 0.0}
}"#;

let snapshot: StatusSnapshot = serde_json::from_str(json).unwrap();
assert!(snapshot.validate().is_ok());
```

Intentionally dependency-light: only `serde`, `serde_json`, and `thiserror`.
No runtime, HTTP, or terminal libraries are included.

Semantic validation runs after serde has allocated owned strings. Callers that
deserialize untrusted bytes directly should impose an input-size limit first;
the protocol crate does not provide a serde-level allocation cap.

Version 2 status responses use a flat `StatusPayloadV2` wrapper. Its optional
`drives`, `cpu_frequency_hz`, `disk_io`, and `network` fields are additive;
missing means unavailable/unsupported/legacy. Disk and network rates are
integer bytes per second, while link capacities are bits per second. The
daemon supplies de-duplicated aggregates and the client preserves detail
records without summing them. CPU frequency is current OS-reported frequency,
not a base or maximum-clock claim; macOS may omit it. Disk capacity is a
separate accounting domain from disk I/O. Network utilization is derived from
the maximum valid directional percentage, so full-duplex Rx and Tx are not
summed, and loopback may be detail-only rather than an aggregate-capacity
member. v1-only and pre-feature v2 payloads remain valid with these fields
absent, while older v2 clients ignore the additive JSON keys.

## Links

- Repository: <https://github.com/eggstack/gregg>
- Project: <https://github.com/eggstack/gregg>

## License

MIT
