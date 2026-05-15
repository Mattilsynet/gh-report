# CHE-0020. Infrastructure-Owned Aggregate Identity

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: CHE-0011, CHE-0013, CHE-0018, CHE-0033

## Context

Every aggregate instance needs an identifier. CHE-0011 and CHE-0013 establish the type and API but neither states the ownership principle. Domain-assigned identity requires uniqueness coordination (UUIDs, sequences, or a registry). Infrastructure-assigned identity — the store assigns the ID during `create()`, the domain never invents IDs, and the aggregate trait has no `id()` method — keeps the domain layer identity-agnostic.

## Decision

The infrastructure layer owns aggregate identity assignment. The
domain layer is identity-agnostic.

Concrete rules:

R1 [5]: Aggregate trait has no id() method; aggregate identity is
  managed by the infrastructure layer
R2 [5]: EventStore::create assigns the aggregate ID via auto-increment
R3 [5]: Callers never invent aggregate IDs; the store is the sole
  source of ID assignment
R4 [5]: If domain logic needs its own identity, store it as a domain
  field populated during the first event's apply

1. **`Aggregate` has no `id()` method.** Aggregate identity is
   managed by the infrastructure layer (event store, command bus,
   gateway). The aggregate itself has no way to query its own ID.
2. **`EventStore::create` assigns the ID.** The store auto-increments
   a `u64` counter and wraps it in `AggregateId(NonZeroU64)`.
   Callers receive the ID as a return value.
3. **Callers never invent IDs.** There is no public constructor
   that takes a raw `u64` and creates an `AggregateId` for use in
   `create()`. The `AggregateId::new(NonZeroU64)` constructor exists
   for the store's internal use and for test helpers, not for domain
   code to generate IDs.
4. **If the domain needs its own identity** (e.g., a `user_id` or
   `order_number`), it stores it as a domain field populated during
   the first event's `apply`. This domain identity is separate from
   the infrastructure `AggregateId`.

```rust
// Aggregate trait — no id() method
pub trait Aggregate: Default + Send + Sync + 'static {
    type Event: DomainEvent;
    fn apply(&mut self, event: &Self::Event);
}

// EventStore::create — store assigns the ID
fn create(&self, events: Vec<Self::Event>)
    -> impl Future<Output = Result<(AggregateId, Vec<EventEnvelope<Self::Event>>), StoreError>> + Send;
```

## Consequences

- **No identity conflicts** — the store guarantees uniqueness via sequential assignment. No UUID collision risk for aggregate IDs (event IDs use UUID v7 per CHE-0033).
- **Aggregate is a pure state machine** — no infrastructure concerns, maximizing testability and keeping the domain runtime-agnostic (CHE-0018).
- **No aggregate self-reference** — an aggregate cannot include its own `AggregateId` in events produced during `handle`; the ID must arrive via the command or a domain field set during a prior `apply`.
- Completes the picture established by CHE-0011 (type), CHE-0013 (API split), and CHE-0018 (domain purity).
