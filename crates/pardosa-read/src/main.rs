#![forbid(unsafe_code)]
mod cli;
mod opaque;
mod render;

use clap::Parser;
use cli::Args;
use pardosa_nats::{JetStreamBackend, JetStreamConfig, RuntimeHandle};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let runtime = tokio::runtime::Runtime::new()?;

    let mut builder = JetStreamConfig::builder()
        .stream_name(args.stream)
        .subject(args.subject)
        .durable_consumer(args.durable_consumer)
        .nats_url(args.nats_url)
        .runtime_handle(RuntimeHandle::from_tokio(runtime.handle().clone()));
    if let Some(creds) = args.creds {
        builder = builder.credentials_path(creds);
    }
    let config = builder.build()?;

    let handle = JetStreamBackend::open(config);
    let records = handle.replay_readonly()?;

    for record in &records {
        let rendered = render::decode_record(record)?;
        let ron = ron::ser::to_string_pretty(&rendered, ron::ser::PrettyConfig::default())?;
        println!("{ron}");
    }

    Ok(())
}
