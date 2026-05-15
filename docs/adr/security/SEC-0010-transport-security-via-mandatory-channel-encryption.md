# SEC-0010. Transport Security via Mandatory Channel Encryption

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Proposed

## Related

References: SEC-0007

## Context

Cloud Run worker pools encrypt outbound GCP traffic via ALTS and provide
IAM-based identity automatically. The gap is the NATS connection — the
one outbound channel the platform does not auto-encrypt. Without TLS on
NATS, events traverse the network in plaintext, undermining SEC-0002
(integrity) and SEC-0007 (confidentiality).

1. **TLS 1.3 on NATS** — `tls_required(true)` in nats::ConnectOptions.
2. **Opportunistic TLS** — fallback to plaintext. Downgrade-vulnerable.
3. **VPC-only** — rely on network isolation. No encryption in transit.

Option 1 chosen: defense in depth requires encryption independent of
network perimeter.

## Decision

All NATS client connections use TLS 1.3+ with no plaintext fallback.
Cloud Run platform handles GCP API transport and HTTP ingress encryption.

R1 [5]: Configure all NATS client connections with TLS 1.3 minimum
  via nats::ConnectOptions requiring tls_required(true)
R2 [5]: Service-to-service NATS connections use mTLS with client
  certificates for mutual authentication at the transport layer
R3 [5]: Cherry-pit-web HTTP endpoints rely on Cloud Run TLS
  termination for ingress encryption rather than application-level TLS

## Consequences

- **Closes the transport gap.** NATS connection — the only unmanaged
  channel — now has explicit TLS governance.
- **Platform leverage.** Cloud Run handles GCP API encryption (ALTS),
  HTTP TLS termination, and IAM identity — no application config needed.
- **Operational complexity.** NATS TLS certificate provisioning and
  rotation become deployment requirements.
- **Development friction.** Local NATS requires self-signed certs or
  a TLS-disable flag scoped to dev profiles.
- **Availability coupling.** Certificate expiry causes total NATS
  connection failure — transport security becomes an availability
  dependency (SEC-0003). Automated rotation and expiry monitoring
  are operational prerequisites.
