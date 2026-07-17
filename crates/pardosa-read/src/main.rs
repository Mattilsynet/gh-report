#![forbid(unsafe_code)]
mod cli;
mod creds_perms;
mod opaque;
mod render;
mod tls_policy;

use clap::Parser;
use cli::Args;
use pardosa_nats::{JetStreamBackend, JetStreamConfig, RuntimeHandle};
use std::error::Error;
use std::process::ExitCode;
use tls_policy::TlsPolicy;

fn main() -> ExitCode {
    let args = Args::parse();

    match tls_policy::evaluate_tls_policy(&args.nats_url, args.allow_plaintext) {
        TlsPolicy::Deny(reason) => {
            eprintln!("error: {reason}");
            return ExitCode::FAILURE;
        }
        TlsPolicy::AllowWithWarning => {
            eprintln!("warning: connecting over plaintext NATS to a non-loopback host");
        }
        TlsPolicy::Allow => {}
    }

    if let Some(creds) = &args.creds {
        creds_perms::warn_if_creds_permissive(creds);
    }

    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), Box<dyn Error>> {
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
