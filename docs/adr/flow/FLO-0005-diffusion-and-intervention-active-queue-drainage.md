# FLO-0005. Diffusion and Intervention: Active Queue Drainage

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted
Parent-cross-domain: PAR-0014 — active-queue-drainage is the FLO-tier expression of PAR-0014's universal directive that backpressure protects writers, by extending passive arrival-refusal with active drainage of items already queued when a queue diffuses into a high-saturation state and cannot self-correct

## Related

References: PAR-0014, FLO-0004, CHE-0041

## Context

Reinertsen Q15 (diffusion — queues drift into high states by random walk and
stay there) and Q16 (intervention — randomness cannot correct a random queue)
make passive backpressure insufficient. PAR-0014 refuses arrivals when the
circuit is open — a passive control. A queue already saturated does not drain
by refusing more arrivals; intervention must actively shed, demote, or purge
existing items. W7 names this: when WIP is high, purge low-value work. This
ADR adds the active-drainage obligation alongside PAR-0014's passive refusal.

## Decision

Each runtime queue declares an intervention threshold and drainage actions to
invoke automatically when saturation is reached; drainage preserves committed
work; every intervention is observable; and policy is per-stream.

R1 [5]: Each runtime queue declares an intervention threshold above which a
  drainage action — purge, demote, or shed — is invoked automatically; passive
  backpressure alone is permitted only when the queue's diffusion
  characteristics make active drainage uneconomic.
R2 [5]: Drainage actions preserve at-least-once delivery for committed work
  (per CHE-0041 idempotency); only uncommitted items or items whose
  cost-of-delay falls below an explicit threshold may be shed without
  acknowledgement.
R3 [8]: Every intervention emits a structured event through the
  queue-telemetry contract (FLO-0004) so operators can audit which work was
  shed, why, and when — drainage is observable, not silent.
R4 [5]: Drainage policy is per-stream rather than global, so high-CoD streams
  retain their items while low-CoD streams shed first under shared saturation;
  cross-stream uniformity is an explicit configuration choice, not the default.

## Consequences

+ becomes easier: high-saturation recovery without operator intervention;
  per-stream sensitivity tuning; downstream consumers see bounded queue depth
  rather than unbounded growth.
− becomes harder: every queue acquires a drainage policy; operators must
  declare what "low value" means per stream.
risks/migration: a wrongly-tuned threshold drains valuable work — R3's
  audit-event obligation is the primary mitigation; R2's at-least-once
  preservation prevents data loss for committed items. Migration: existing
  PAR-0014-only queues acquire drainage at next touch; default policy can be
  "drain nothing" (intervention disabled) for streams whose diffusion
  behaviour is benign.
