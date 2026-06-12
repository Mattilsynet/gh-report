# PAR-0014. Backpressure and Circuit Breaker

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: C
Status: Deprecated

## Retirement

Retired without a PGN successor: backpressure and circuit-breaker policy for RwLock-across-async publish was a runtime concurrency design, and PGN backends own async internally without restating this circuit-breaker contract.
