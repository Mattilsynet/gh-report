//! prod-loc — count Rust production lines, excluding `#[cfg(test)]`
//! modules and `tests/` directories.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use prod_loc::{AggregateReport, FileReport, analyse_file, walk};

fn main() -> ExitCode {
    let mut path: Option<PathBuf> = None;
    let mut details = false;

    let args = std::env::args().skip(1);
    for a in args {
        match a.as_str() {
            "--details" => details = true,
            "-h" | "--help" => {
                print_help();
                return ExitCode::from(0);
            }
            other if other.starts_with('-') => {
                eprintln!("unknown flag: {other}");
                print_help();
                return ExitCode::from(2);
            }
            other => {
                if path.is_some() {
                    eprintln!("only one PATH accepted; got extra: {other}");
                    return ExitCode::from(2);
                }
                path = Some(PathBuf::from(other));
            }
        }
    }

    let Some(path) = path else {
        eprintln!("usage: prod-loc <PATH> [--details]");
        return ExitCode::from(2);
    };

    if !path.exists() {
        eprintln!("path does not exist: {}", path.display());
        return ExitCode::from(1);
    }

    // Single-file mode: honour the rules verbatim. If it's not .rs, error.
    // If it has a `tests/` ancestor, the user asked for it explicitly —
    // report it but flag as excluded by the `tests/` rule.
    let files = if path.is_file() {
        if path.extension().is_none_or(|e| e != "rs") {
            eprintln!("not a .rs file: {}", path.display());
            return ExitCode::from(1);
        }
        vec![path.clone()]
    } else {
        walk::collect_rs_files(&path)
    };

    if files.is_empty() {
        eprintln!("no .rs files found under: {}", path.display());
        return ExitCode::from(1);
    }

    let mut agg = AggregateReport::default();
    for f in files {
        let report = if walk::path_has_tests_component(&f) {
            // Explicit single-file path inside tests/ — count totals but
            // record as fully test-side.
            let source = std::fs::read_to_string(&f)
                .expect("file in tests/ must be readable; check permissions");
            let total = prod_loc::count_lines(&source);
            FileReport {
                path: f.clone(),
                total_lines: total,
                production_lines: 0,
                test_lines: total,
                excluded_reason: Some("tests/ dir"),
            }
        } else {
            analyse_file(&f)
        };
        agg.total_production += report.production_lines;
        agg.total_test += report.test_lines;
        agg.files.push(report);
    }

    if details {
        for r in &agg.files {
            let reason = r.excluded_reason.unwrap_or("-");
            println!(
                "FILE\t{}\t{}\t{}\t{}\t{}",
                r.path.display(),
                r.production_lines,
                r.test_lines,
                r.total_lines,
                reason
            );
        }
    }
    println!("PROD-LOC\t{}", agg.total_production);
    println!("TEST-LOC\t{}", agg.total_test);
    println!("FILES\t{}", agg.total_files());

    ExitCode::from(0)
}

fn print_help() {
    println!(
        "prod-loc — count Rust production lines, excluding tests.\n\
         \n\
         USAGE:\n    prod-loc <PATH> [--details]\n\
         \n\
         FLAGS:\n\
         \x20 --details        print per-file FILE\\t<path>\\t<prod>\\t<test>\\t<total>\\t<reason>\n\
         \x20 -h, --help       show this help\n\
         \n\
         OUTPUT (tab-separated):\n\
         \x20 PROD-LOC\\t<n>\n\
         \x20 TEST-LOC\\t<n>\n\
         \x20 FILES\\t<n>\n\
         \n\
         EXIT: 0 measurement complete; 1 path/file errors; 2 bad args."
    );
}
