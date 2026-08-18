//! Local network interface discovery for `configprint` display.
//!
//! When the configured bind host is a wildcard (`0.0.0.0` or `::`),
//! it does not name a single address a remote client can dial. To
//! produce a useful single-line output, the daemon resolves the
//! wildcard to the host's primary local IP address.
//!
//! The resolution uses a transient UDP socket bound to the unspecified
//! address and `connect()`ed to a fixed stable destination. The kernel
//! resolves the route locally and selects the source IP. No packets
//! are transmitted: `connect()` on a UDP socket only sets the default
//! destination and surfaces the local address the kernel would use.
//!
//! This is consistent with the `configprint` constraint that it must
//! not probe, bind, mutate config, or manage services: the helper
//! performs no network I/O, no daemon bind, and no side effects beyond
//! the resolution itself.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, UdpSocket};

/// Probe destination for IPv4 resolution. The routed destination is
/// irrelevant as long as the kernel can resolve a route; the address
/// below is a stable, widely-known IPv4 endpoint so the route lookup
/// has a deterministic answer on hosts with a default route.
const IPV4_PROBE: (Ipv4Addr, u16) = (Ipv4Addr::new(8, 8, 8, 8), 80);

/// Probe destination for IPv6 resolution. `2001:4860:4860::8888` is
/// the well-known Google DNS IPv6 endpoint.
const IPV6_PROBE: (Ipv6Addr, u16) = (
    Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888),
    80,
);

/// Return the local IPv4 address the kernel would use to reach an
/// arbitrary IPv4 destination. Returns `None` if the host has no IPv4
/// route, the bind fails, or the kernel cannot resolve the probe.
#[must_use]
pub fn local_ipv4_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(IPV4_PROBE).ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}

/// Return the local IPv6 address the kernel would use to reach an
/// arbitrary IPv6 destination. Returns `None` if the host has no IPv6
/// route, the IPv6 stack is unavailable, or the kernel cannot resolve
/// the probe.
#[must_use]
pub fn local_ipv6_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(IPV6_PROBE).ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ipv4_probe_is_the_expected_destination() {
        assert_eq!(IPV4_PROBE.0, Ipv4Addr::new(8, 8, 8, 8));
        assert_eq!(IPV4_PROBE.1, 80);
    }

    #[test]
    fn local_ipv6_probe_is_the_expected_destination() {
        assert_eq!(
            IPV6_PROBE.0,
            Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888)
        );
        assert_eq!(IPV6_PROBE.1, 80);
    }

    #[test]
    fn local_ipv4_resolution_returns_some_when_route_available() {
        // Real-environment smoke: the function only returns None on
        // machines with no IPv4 route. We assert the Ipv4Addr shape
        // when it does return Some.
        if let Some(addr) = local_ipv4_address() {
            assert!(matches!(addr, IpAddr::V4(_)), "expected IPv4, got {addr}");
        }
    }

    #[test]
    fn local_ipv6_resolution_returns_some_when_route_available() {
        if let Some(addr) = local_ipv6_address() {
            assert!(matches!(addr, IpAddr::V6(_)), "expected IPv6, got {addr}");
        }
    }
}
