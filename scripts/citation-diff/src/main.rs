//! citation-diff — verify an ADR markdown edit preserves every citation token.
//!
//! Usage: citation-diff <pre-file> <post-file>
//!
//! Output (stdout, tab-separated, one record per line):
//!   MATCH\t<token>\t<pre_count>\t<post_count>
//!   DECREASED\t<token>\t<pre_count>\t<post_count>   (still present, count dropped — prose compression, advisory)
//!   MISSING_AFTER\t<token>\t<pre_count>\t0           (token deleted entirely — gate failure)
//!   EXTRA_AFTER\t<token>\t0\t<post_count>
//!
//! Stderr summary: `summary: pre_tokens=<n> matched=<n> decreased=<n> missing=<n> extra=<n>`
//!
//! Exit codes:
//!   0 — no MISSING_AFTER (token deletions); DECREASED and EXTRA_AFTER allowed.
//!       Prose compression around repeated citations is normal and not a gate failure;
//!       only a token's complete disappearance from the post-edit file is a hard miss.
//!   1 — one or more MISSING_AFTER (a citation token was deleted entirely)
//!   2 — I/O error reading either file

#![forbid(unsafe_code)]

use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::process::ExitCode;

const PATTERNS: &[&str] = &[
    r"\bCHE-\d{4}(?::R\d+)?\b",
    r"\bAFM-\d{4}\b",
    r"\bFLO-\d{4}\b",
    r"\bSEC-\d{4}\b",
    r"\bPAR-\d{4}\b",
    r"\bCOM-\d{4}\b",
    r"\bGEN-\d{4}\b",
    r"\bGND-\d{4}\b",
    r"\bRST-\d{4}\b",
    r"\bCHE-NNNN\b",
    r"\badr-fmt-[a-z0-9]+\b",
    r"\b\S+\.(?:md|rs|toml)\b",
];

#[derive(Debug)]
enum Verdict {
    Match,
    Decreased,
    MissingAfter,
    ExtraAfter,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: citation-diff <pre-file> <post-file>");
        return ExitCode::from(2);
    }

    let pre_path = &args[0];
    let post_path = &args[1];

    let pre = match std::fs::read_to_string(pre_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read pre-file {pre_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let post = match std::fs::read_to_string(post_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read post-file {post_path}: {e}");
            return ExitCode::from(2);
        }
    };

    let regexes: Vec<Regex> = PATTERNS
        .iter()
        .map(|p| Regex::new(p).expect("citation-diff: built-in pattern must compile"))
        .collect();

    let pre_tokens = scan(&pre, &regexes);
    let post_tokens = scan(&post, &regexes);

    // Deterministic ordering: sort by token (BTreeMap union).
    let mut all: BTreeMap<&str, ()> = BTreeMap::new();
    for k in pre_tokens.keys() {
        all.insert(k.as_str(), ());
    }
    for k in post_tokens.keys() {
        all.insert(k.as_str(), ());
    }

    let mut matched = 0_usize;
    let mut decreased = 0_usize;
    let mut missing = 0_usize;
    let mut extra = 0_usize;

    for token in all.keys() {
        let pre_n = pre_tokens.get(*token).copied().unwrap_or(0);
        let post_n = post_tokens.get(*token).copied().unwrap_or(0);
        let verdict = if pre_n == 0 {
            Verdict::ExtraAfter
        } else if post_n == 0 {
            // Token vanished entirely — hard miss (citation deleted).
            Verdict::MissingAfter
        } else if post_n >= pre_n {
            Verdict::Match
        } else {
            // Token still present, occurrence count dropped — prose
            // compression around a repeated citation. Advisory only.
            Verdict::Decreased
        };
        match verdict {
            Verdict::Match => {
                matched += 1;
                println!("MATCH\t{token}\t{pre_n}\t{post_n}");
            }
            Verdict::Decreased => {
                decreased += 1;
                println!("DECREASED\t{token}\t{pre_n}\t{post_n}");
            }
            Verdict::MissingAfter => {
                missing += 1;
                println!("MISSING_AFTER\t{token}\t{pre_n}\t{post_n}");
            }
            Verdict::ExtraAfter => {
                extra += 1;
                println!("EXTRA_AFTER\t{token}\t0\t{post_n}");
            }
        }
    }

    eprintln!(
        "summary: pre_tokens={} matched={} decreased={} missing={} extra={}",
        pre_tokens.len(),
        matched,
        decreased,
        missing,
        extra,
    );

    if missing > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn scan(text: &str, regexes: &[Regex]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for re in regexes {
        for m in re.find_iter(text) {
            *counts.entry(m.as_str().to_string()).or_insert(0) += 1;
        }
    }
    counts
}
