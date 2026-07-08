use std::collections::HashSet;
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;

pub(crate) struct AdrRef {
    pub(crate) id: String,
    pub(crate) title: String,
}

pub(crate) fn governing_adrs(crate_name: &str) -> Vec<AdrRef> {
    let Ok(output) = Command::new("adr-fmt")
        .arg("--context")
        .arg(crate_name)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    distinct_headings(&stdout)
}

fn heading_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?m)^### ([A-Z]{2,6}-\d{4})\. (.+)$").expect("heading pattern is valid")
    })
}

fn distinct_headings(text: &str) -> Vec<AdrRef> {
    let pattern = heading_pattern();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for capture in pattern.captures_iter(text) {
        let id = capture[1].to_string();
        let title = capture[2].trim().to_string();
        if seen.insert(id.clone()) {
            out.push(AdrRef { id, title });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::distinct_headings;

    #[test]
    fn extracts_unique_governing_adr_headings_in_first_seen_order() {
        let sample = "\
intro line, not a heading
### RST-0001. Pinned Stable Toolchain with MSRV Contract
- some rule [RST-0001:R1:L5]
### -0000. Unclaimed Rules
- an unclaimed rule
### RST-0001. Pinned Stable Toolchain with MSRV Contract
- duplicate section, same id
### PGN-0001. Pardosa Crate Rings and Authority Boundaries
- another rule
";

        let headings = distinct_headings(sample);

        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].id, "RST-0001");
        assert_eq!(
            headings[0].title,
            "Pinned Stable Toolchain with MSRV Contract"
        );
        assert_eq!(headings[1].id, "PGN-0001");
    }

    #[test]
    fn empty_input_yields_no_headings() {
        assert!(distinct_headings("").is_empty());
    }
}
