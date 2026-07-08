#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Projects the workspace's crates into fenced Obsidian vault notes")]
struct Cli {
    #[arg(long, default_value = ".ooda/architect/preview")]
    out: PathBuf,

    #[arg(long)]
    subfolder: Option<PathBuf>,

    #[arg(long, default_value = ".")]
    workspace_root: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = architect::Config::new(cli.workspace_root, cli.out, cli.subfolder);
    match architect::run(&config) {
        Ok(report) => {
            println!("wrote {} note(s):", report.written.len());
            for path in &report.written {
                println!("  {}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("architect: {error}");
            ExitCode::FAILURE
        }
    }
}
