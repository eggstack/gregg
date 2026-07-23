//! `greggd` skeleton.
//!
//! Phase 2 adds the Linux collector module. Phase 3 will add the macOS
//! collector. Phase 4 wires the sampler and HTTP server. Until then the
//! binary only reports its protocol schema version so the workspace remains
//! packageable.

use gregg_protocol::SCHEMA_VERSION_V1;

fn main() {
    println!("greggd skeleton (protocol schema version {SCHEMA_VERSION_V1})");
}
