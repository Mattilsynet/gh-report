//! track4-verify CLI entry point.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use track4_verify::criteria::CRITERIA;
use track4_verify::{Context, Verdict};

fn main() -> ExitCode {
    let mut eventstore_ceiling: usize = 8;
    let mut strict_docs = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--eventstore-ceiling" => {
                let v = args
                    .next()
                    .expect("--eventstore-ceiling requires a numeric argument");
                eventstore_ceiling = v
                    .parse()
                    .expect("--eventstore-ceiling value must be a usize");
            }
            "--strict-docs" => strict_docs = true,
            "-h" | "--help" => {
                print_help();
                return ExitCode::from(0);
            }
            other => {
                eprintln!("unknown arg: {other}");
                print_help();
                return ExitCode::from(2);
            }
        }
    }

    let workspace_root = discover_workspace_root();
    let ctx = Context {
        workspace_root,
        eventstore_ceiling,
        strict_docs,
    };

    let start = Instant::now();
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut manual = 0usize;

    for c in CRITERIA {
        let r = (c.runner)(&ctx);
        match r.verdict {
            Verdict::Pass => pass += 1,
            Verdict::Fail => fail += 1,
            Verdict::Manual => manual += 1,
        }
        // Tagged record: CRITERION\t<num>\t<short>\t<verdict>\t<metric>\t<note>
        println!(
            "CRITERION\t{}\t{}\t{}\t{}\t{}",
            c.num,
            c.short_name,
            r.verdict.tag(),
            r.metric,
            if r.note.is_empty() {
                "-".to_string()
            } else {
                r.note
            },
        );
    }

    let total_ms = start.elapsed().as_millis();
    println!(
        "SUMMARY\t{}\t{}\t{}\t{}\t{}",
        CRITERIA.len(),
        pass,
        fail,
        manual,
        total_ms
    );

    if fail == 0 {
        ExitCode::from(0)
    } else {
        ExitCode::from(1)
    }
}

fn discover_workspace_root() -> PathBuf {
    // Walk up from CWD until a Cargo.toml with `[workspace]` is found.
    let mut cwd = std::env::current_dir().expect("current dir must be readable");
    loop {
        let candidate = cwd.join("Cargo.toml");
        if candidate.is_file() {
            let content = std::fs::read_to_string(&candidate)
                .expect("Cargo.toml at candidate must be readable");
            if content.contains("[workspace]") && content.contains("members") {
                return cwd;
            }
        }
        if !cwd.pop() {
            panic!(
                "could not find workspace Cargo.toml walking up from CWD; \
                 run track4-verify from inside the solon workspace"
            );
        }
    }
}

fn print_help() {
    println!(
        "track4-verify — mechanical verifier for Phase 2 v2 Track 4 exit criteria\n\
         \n\
         USAGE:\n    track4-verify [--eventstore-ceiling N] [--strict-docs]\n\
         \n\
         FLAGS:\n\
         \x20 --eventstore-ceiling N   max EventStore mentions (default 8)\n\
         \x20 --strict-docs            fail on #12 heuristic miss (default: MANUAL)\n\
         \x20 -h, --help               show this help\n\
         \n\
         OUTPUT (tab-separated):\n\
         \x20 CRITERION\\t<num>\\t<short>\\t<PASS|FAIL|MANUAL>\\t<metric>\\t<note>\n\
         \x20 SUMMARY\\t<total>\\t<pass>\\t<fail>\\t<manual>\\t<duration_ms>\n\
         \n\
         EXIT: 0 if no FAILs, 1 otherwise."
    );
}
