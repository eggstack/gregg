# Security Policy

## Supported versions

| Version | Supported |
| --- | --- |
| 1.0.x | Yes |

## Reporting a vulnerability

If you discover a security vulnerability in gregg, please report it
responsibly. **Do not open a public GitHub issue for security reports.**

Instead, please email dbowman91@proton.me. Include:

- A description of the vulnerability
- Steps to reproduce
- Affected versions
- Any potential impact assessment

## Scope

gregg is designed for **private-network, read-only** system observation. It
does not provide:

- TLS or HTTPS transport
- Authentication or authorization
- Rate limiting or DDoS protection
- Input sanitization beyond protocol validation

Exposing greggd directly to untrusted networks is outside the project's
design intent and security model. Users who deploy greggd on accessible
networks are responsible for network-level controls (firewalls, VPNs,
reverse proxies).

The default bind address is loopback (`127.0.0.1`), so a default install is not
reachable from other hosts. A wildcard (`0.0.0.0`, `::`) or concrete non-loopback
address is a supported configuration for LAN monitoring, is never a default, and
logs one startup warning naming the address and the fact that the API is
unauthenticated.

## Response timeline

We will acknowledge receipt of a report within 72 hours and aim to provide
an initial assessment within one week. Critical vulnerabilities in the
published crates will be addressed with a patch release and a yanked
version if necessary.
