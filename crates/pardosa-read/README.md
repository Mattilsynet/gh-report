# pardosa-read

Read-only RON reader for pardosa events stored in JetStream. Built for
agent-driven debugging against production NATS (mission
`pardosa-read-cli-1752739200`).

## Read-only guarantee

`pardosa-read` constructs only
[`JetStreamHandle::replay_readonly`](../pardosa-nats/src/handle.rs)
reads. It never calls `append`, `sync`, `get_or_create_stream`, or
`update_stream`; a missing stream surfaces as a typed error instead of
being provisioned. This is a structural guarantee (PGN-0008:R8
rehydrate-only-errors-on-never-created), not just an operator
convention.

## Operator run recipe

Supply a NATS credentials file and the target stream/subject
explicitly — the tool does not enumerate streams.

```sh
cargo run --bin pardosa-read -- \
  --nats-url tls://<your-nats-host>:4222 \
  --creds /path/to/operator.creds \
  --stream gh-report--v18 \
  --subject gh-report.v18.events
```

Org-scoped variant (per-org subject):

```sh
cargo run --bin pardosa-read -- \
  --nats-url tls://<your-nats-host>:4222 \
  --creds /path/to/operator.creds \
  --stream gh-report--v18 \
  --subject gh-report.v18.org.events
```

Each replay record is printed to stdout as a RON document with the
envelope frame decoded structurally (`event_id`, `fiber_id`,
`detached`, `precursor`, `precursor_hash_hex`) and the `domain_event`
body rendered as opaque hex (`domain_event_hex`), since the wire
format is schema-driven and this tool does not link any consumer's
concrete event types.

Never commit a real `.creds` file or a real NATS URL to this
repository; the invocation above uses placeholders only.

## CLI arguments

| Flag | Required | Notes |
|---|---|---|
| `--nats-url` | yes | e.g. `tls://<host>:4222` |
| `--creds` | no | path to a NATS credentials file |
| `--stream` | yes | JetStream stream name |
| `--subject` | yes | single subject (no wildcards) |
| `--durable-consumer` | no | defaults to `pardosa-read-ro`; `replay_readonly` uses an ephemeral consumer internally, so this is largely unused but required non-empty by `JetStreamConfig` |
