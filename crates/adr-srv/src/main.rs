#![forbid(unsafe_code)]

//! adr-srv binary stub. Track 3.2 skeleton — not a server yet.

use std::path::PathBuf;

fn main() {
    println!("adr-srv skeleton");
    // The binary, like any consumer, must supply a marker dir. Use CWD
    // and let adr-fmt's walk-up locate the workspace adr-fmt.toml.
    let cwd: PathBuf = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match adr_srv::surface_probe(&cwd) {
        Ok(root) => println!("corpus root: {}", root.display()),
        Err(e) => {
            eprintln!("surface_probe failed: {e}");
            std::process::exit(1);
        }
    }
}
