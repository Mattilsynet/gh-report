#![crate_type = "lib"]
//! Fixture for lints the workspace policy excludes.
//!
//! Every construct here is a live witness for one excluded lint: with the
//! matching `allow` entry present the lint must be silent, and with that one
//! entry removed (the positive-control pass) it must fire.

struct Hidden {
    field: usize,
}

fn hidden(value: usize) -> Hidden {
    Hidden { field: value }
}

pub struct ControlCell {
    pub excluded_total: usize,
    pub excluded_formatted: String,
}

pub enum Outcome {
    Accepted,
    Rejected,
}

pub struct RepositoryFlags {
    pub archived: bool,
    pub has_issues: bool,
    pub fork: bool,
    pub is_empty: bool,
}

pub fn shift(base: usize, delta: usize) -> usize {
    let total = base + delta;
    total
}

pub fn ratio(numerator: f64, denominator: f64) -> f64 {
    numerator / denominator
}

pub fn classify(flag: bool) -> Outcome {
    if flag {
        Outcome::Accepted
    } else {
        Outcome::Rejected
    }
}

pub fn absolute(value: i64) -> std::string::String {
    std::string::String::from(if value < 0 { "neg" } else { "pos" })
}

pub fn label(value: &str) -> String {
    value.to_string()
}

pub fn first_field(value: usize) -> usize {
    hidden(value).field
}

pub fn shorten(value: Option<u32>) -> Option<u32> {
    let inner = value?;
    Some(inner)
}

pub fn swap_pair(left: &mut u32, right: &mut u32) {
    std::mem::swap(left, right);
}
