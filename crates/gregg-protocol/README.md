# gregg-protocol

Versioned JSON wire types, metric capabilities, and identity structures shared
by the gregg daemon and client.

## Usage

```rust
use gregg_protocol::StatusSnapshot;

let json = r#"{
  "schema_version": 1,
  "observed_at_unix_ms": 1700000000000,
  "sample_interval_ms": 5000,
  "capabilities": {"cpu_iowait": true},
  "system": {"name": "deadpool", "hostname": "deadpool.local",
    "os_name": "linux", "os_version": "24.04",
    "kernel_name": "Linux", "kernel_release": "6.8.0",
    "architecture": "x86_64"},
  "cpu": {"usage_pct": 25.2, "iowait_pct": 0.4},
  "load": {"load_1m": 1.32, "load_5m": 0.91, "load_15m": 0.62},
  "memory": {"total_bytes": 16777216000, "used_bytes": 6343841792, "usage_pct": 37.8},
  "swap": {"total_bytes": 4294967296, "used_bytes": 0, "usage_pct": 0.0}
}"#;

let snapshot: StatusSnapshot = serde_json::from_str(json).unwrap();
assert!(snapshot.validate().is_ok());
```

Intentionally dependency-light: only `serde`, `serde_json`, and `thiserror`.
No runtime, HTTP, or terminal libraries are included.

## Links

- Repository: <https://github.com/eggstack/gregg>
- Project: <https://github.com/eggstack/gregg>

## License

MIT
