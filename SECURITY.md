# Security Policy

## Supported versions

| Version | Supported |
| --- | --- |
| 1.0.x | Yes |

## Reporting a vulnerability

If you discover a security vulnerability in gregg, please report it
responsibly. **Do not open a public GitHub issue for security reports.**

Instead, please email the maintainers at the address listed in the
repository ownership. Include:

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

## Response timeline

We will acknowledge receipt of a report within 72 hours and aim to provide
an initial assessment within one week. Critical vulnerabilities in the
published crates will be addressed with a patch release and a yanked
version if necessary.
