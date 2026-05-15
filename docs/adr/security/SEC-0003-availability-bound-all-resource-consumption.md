# SEC-0003. Availability — Bound All Resource Consumption

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: SEC-0001, GND-0001, GND-0005

## Context

Availability ensures timely access despite failures or attacks.
The primary CISQ threat is denial of service through resource
exhaustion — unbounded allocations, infinite loops, or recursive
decompression. Every allocation, iteration, and recursion driven
by external input must have an upper bound to prevent a single
malicious input from consuming all available resources.

## Decision

Bound all resource consumption driven by external input. No
unbounded allocation, iteration, or recursion from untrusted data.

R1 [5]: Every allocation sized by external input has a
  configurable maximum enforced before allocation
R2 [5]: Every loop or recursion driven by external input has a
  depth or iteration limit
R3 [5]: Backpressure mechanisms exist at every ingestion point
  to shed load when capacity is exceeded
R4 [6]: Default resource limits are documented in code comments
  adjacent to the limit constants

## Consequences

No single input can exhaust memory, stack, or CPU. Resource limits
are explicit and configurable rather than implicit in data structure
capacity. The trade-off is rejecting legitimately large inputs that
exceed limits, requiring limits to be tuned for expected workloads.
