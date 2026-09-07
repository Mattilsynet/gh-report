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
        .timeout_observer(pardosa_nats::TimeoutObserver::new(|error| {
            eprintln!("error: {error}");
        }))
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

#[cfg(test)]
mod timeout_tests {
    #[test]
    fn cli_captures_production_connect_timeout() {
        const CHILD_URL: &str = "PARDOSA_TIMEOUT_TEST_URL";
        if let Ok(url) = std::env::var(CHILD_URL) {
            let result = super::run(super::Args {
                nats_url: url,
                creds: None,
                stream: "timeout".into(),
                subject: "timeout".into(),
                durable_consumer: "timeout".into(),
                allow_plaintext: false,
            });
            assert!(result.is_err());
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "timeout_tests::cli_captures_production_connect_timeout",
                "--nocapture",
            ])
            .env(
                CHILD_URL,
                format!("nats://{}", listener.local_addr().unwrap()),
            )
            .env("PARDOSA_NATS_OPERATION_TIMEOUT_SECS", "1")
            .output()
            .unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("error: operation deadline expired during connect:"),
            "{stderr}"
        );
        assert!(stderr.contains("remote outcome may be unknown"));
    }
}
