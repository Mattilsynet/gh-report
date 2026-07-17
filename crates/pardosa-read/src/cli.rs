use clap::Parser;
use std::path::PathBuf;

/// Read-only RON reader for pardosa events stored in `JetStream`.
/// Constructs only
/// [`pardosa_nats::JetStreamHandle::replay_readonly`] reads: never
/// appends, provisions, or mutates a stream/consumer/message.
#[derive(Parser, Debug)]
#[command(
    name = "pardosa-read",
    about = "Read-only RON reader for pardosa JetStream events"
)]
pub struct Args {
    #[arg(long, env = "PARDOSA_READ_NATS_URL")]
    pub nats_url: String,

    #[arg(long, env = "PARDOSA_READ_NATS_CREDS")]
    pub creds: Option<PathBuf>,

    #[arg(long)]
    pub stream: String,

    #[arg(long)]
    pub subject: String,

    #[arg(long, default_value = "pardosa-read-ro")]
    pub durable_consumer: String,

    #[arg(long, default_value_t = false)]
    pub allow_plaintext: bool,
}
