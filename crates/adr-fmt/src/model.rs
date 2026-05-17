use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

/// A domain directory (e.g., `docs/adr/cherry/` with prefix `CHE`).
#[derive(Debug, Clone)]
pub struct DomainDir {
    pub path: PathBuf,
    pub prefix: String,
    pub name: String,
}

/// A tagged rule extracted from the Decision section.
///
/// Format in ADR: `R1 [5]: Rule text here`
/// Global identifier: `CHE-0042:R1:L5`
#[derive(Debug, Clone)]
pub struct TaggedRule {
    pub id: String,
    pub text: String,
    /// Meadows leverage layer (1-12). 0 indicates unparsed/invalid.
    pub layer: u8,
    /// 1-indexed line number where this rule appears in the source file.
    pub line: usize,
}

/// Map a Meadows leverage layer (1-12) to the corresponding tier.
///
/// Mapping: S=1-3, A=4, B=5-6, C=7-8, D=9-12.
/// Returns `None` for layer 0 or >12 (invalid).
#[must_use]
pub fn layer_to_tier(layer: u8) -> Option<Tier> {
    match layer {
        1..=3 => Some(Tier::S),
        4 => Some(Tier::A),
        5..=6 => Some(Tier::B),
        7..=8 => Some(Tier::C),
        9..=12 => Some(Tier::D),
        _ => None,
    }
}

/// Composite ADR identifier: prefix + number (e.g., CHE-0042).
///
/// # Invariants (parser-produced values only)
///
/// Values produced by [`parse_adr_id`] or [`parse_adr_id_from_filename_stem`]
/// satisfy:
/// - `prefix.len() ∈ 2..=4`
/// - every byte of `prefix` is `b'A'..=b'Z'` (ASCII uppercase)
/// - `number ∈ 0..=9999` (encoded as exactly 4 digits via `Display`)
///
/// Values produced by [`AdrRecord::default`] (and other manual
/// construction sites) do not satisfy these invariants — `prefix`
/// is the empty string. Manual construction is restricted to
/// sentinel/uninitialized contexts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AdrId {
    pub prefix: String,
    pub number: u16,
}

impl fmt::Display for AdrId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:04}", self.prefix, self.number)
    }
}

/// Parsed ADR record with all metadata and line numbers.
///
/// Bool fields represent independent parser-detected facts about the
/// record (presence of various sections/headers); collapsing into a
/// flags enum would lose the per-field rustdoc surface and obscure
/// individual access patterns in rules.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent section-presence/position flag with its own rustdoc surface; collapsing into a flags enum loses per-field access patterns used in rules and obscures the parser's individual probes"
)]
pub struct AdrRecord {
    pub id: AdrId,
    pub file_path: PathBuf,
    pub title: Option<String>,
    pub title_line: usize,
    pub date: Option<String>,
    pub last_reviewed: Option<String>,
    pub tier: Option<Tier>,
    pub status: Option<Status>,
    pub status_line: usize,
    pub status_raw: Option<String>,
    pub relationships: Vec<Relationship>,
    pub has_related: bool,
    pub has_context: bool,
    pub has_decision: bool,
    pub has_consequences: bool,
    pub has_retirement: bool,
    /// True when the ADR file lives in the stale archive directory.
    pub is_stale: bool,
    /// True when status was parsed from the legacy `## Status` section
    /// (not the `Status:` preamble metadata field).
    pub status_from_section: bool,
    pub max_code_block_lines: usize,
    /// 1-indexed line number of the opening fence of the largest code
    /// block. 0 if no code blocks exist.
    pub max_code_block_line: usize,
    /// Ordered list of H2 section names as they appear in the file.
    pub section_order: Vec<String>,
    /// Word count per H2 section (section name → count). Code blocks
    /// are excluded from the count.
    pub section_word_counts: HashMap<String, usize>,
    /// Crates associated with this ADR via `Crates:` metadata field.
    pub crates: Vec<String>,
    /// Tagged rules extracted from the Decision section
    /// (`RN [L]: text` pattern). Empty when no tagged rules found.
    pub decision_rules: Vec<TaggedRule>,
    /// Cross-domain parent exception declared in the preamble via
    /// `Parent-cross-domain: PREFIX-NNNN — reason`. When present and
    /// matching the first `References:` target, suppresses L011
    /// (cross-domain parent edge) for that relationship.
    ///
    /// The string carries the parsed target ID. The reason text is
    /// preserved in `parent_cross_domain_reason` for output.
    pub parent_cross_domain: Option<AdrId>,
    /// Free-text reason accompanying `parent_cross_domain`. Empty
    /// when the field has only the target ID with no reason.
    /// Currently parsed but not rendered; preserved for future tree
    /// or lint surfacing (see AFM-0024).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "AFM-0024 cross-domain parent surfacing deferred (tree/lint rendering not yet wired); field is parsed and ready, fail-closes via #[expect] when the surface lands. Wrapped in cfg_attr(not(test)) because test fixtures write the field, which silences dead_code under --all-targets."
        )
    )]
    pub parent_cross_domain_reason: String,
}

impl AdrRecord {
    /// True if this ADR declares itself as a tree root via `Root: OWN-ID`.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.relationships
            .iter()
            .any(|r| r.verb == RelVerb::Root && r.target == self.id)
    }
}

impl Default for AdrRecord {
    fn default() -> Self {
        Self {
            id: AdrId {
                prefix: String::new(),
                number: 0,
            },
            file_path: PathBuf::new(),
            title: None,
            title_line: 0,
            date: None,
            last_reviewed: None,
            tier: None,
            status: None,
            status_line: 0,
            status_raw: None,
            relationships: Vec::new(),
            has_related: false,
            has_context: false,
            has_decision: false,
            has_consequences: false,
            has_retirement: false,
            is_stale: false,
            status_from_section: false,
            max_code_block_lines: 0,
            max_code_block_line: 0,
            section_order: Vec::new(),
            section_word_counts: HashMap::new(),
            crates: Vec::new(),
            decision_rules: Vec::new(),
            parent_cross_domain: None,
            parent_cross_domain_reason: String::new(),
        }
    }
}

/// ADR tier classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    S,
    A,
    B,
    C,
    D,
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::S => f.write_str("S"),
            Self::A => f.write_str("A"),
            Self::B => f.write_str("B"),
            Self::C => f.write_str("C"),
            Self::D => f.write_str("D"),
        }
    }
}

impl Tier {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "S" => Some(Self::S),
            "A" => Some(Self::A),
            "B" => Some(Self::B),
            "C" => Some(Self::C),
            "D" => Some(Self::D),
            _ => None,
        }
    }

    /// Human-readable tier name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::S => "Intent",
            Self::A => "Self-organization",
            Self::B => "Design",
            Self::C => "Feedbacks",
            Self::D => "Parameters",
        }
    }

    /// Tier meaning and scope description.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::S => {
                "Paradigm, goals, or governance — changing reshapes the \
                        system's purpose and every tier below it."
            }
            Self::A => {
                "Extension points and structural evolvability — changing \
                        alters what the system can become."
            }
            Self::B => {
                "Type contracts, API boundaries, and information flows — \
                        changing requires coordinated updates across crates."
            }
            Self::C => {
                "Runtime behaviour and interaction dynamics — changing \
                        requires coordinated call-site updates."
            }
            Self::D => {
                "Implementation details and tooling configuration — \
                        changing affects only crate internals."
            }
        }
    }

    /// Stability expectation for this tier.
    #[must_use]
    pub fn stability(self) -> &'static str {
        match self {
            Self::S => "Immutable post-1.0",
            Self::A => "Near-immutable; changes require RFC-level discussion",
            Self::B => "Stable; changes documented via git history",
            Self::C => "Stable; changes require integration testing",
            Self::D => "Mutable; may be superseded freely",
        }
    }

    /// All tier variants in order.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::S, Self::A, Self::B, Self::C, Self::D]
    }

    /// Numeric rank for sorting (S=0, A=1, ... D=4).
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::S => 0,
            Self::A => 1,
            Self::B => 2,
            Self::C => 3,
            Self::D => 4,
        }
    }

    /// Tier-scaling factor for word count and rule limits.
    ///
    /// S-tier decisions are broad (paradigm-level) and get more room.
    /// D-tier decisions are narrow (parameters) and should be tighter.
    /// Applied as a multiplier to `max_words` and `max_rules` base values.
    #[must_use]
    pub fn factor(self) -> f64 {
        match self {
            Self::S => 1.5,
            Self::A => 1.2,
            Self::B => 1.0,
            Self::C => 0.8,
            Self::D => 0.6,
        }
    }

    /// Tier-scaled minimum word count for prose sections.
    ///
    /// Higher-tier ADRs need more substance; lower-tier can be brief.
    #[must_use]
    // Each tier is a distinct semantic knob; coincident values are
    // calibration, not a request to collapse arms.
    #[expect(
        clippy::match_same_arms,
        reason = "each tier is an independently-tunable calibration knob; coincident values are accidental, not semantic equivalence — collapsing arms would erase the calibration intent"
    )]
    pub fn min_words(self) -> u64 {
        match self {
            Self::S => 15,
            Self::A => 12,
            Self::B => 10,
            Self::C => 7,
            Self::D => 7,
        }
    }

    /// Tier-scaled maximum reference count (References: targets only).
    ///
    /// Root and Supersedes are structural, not content dependencies,
    /// and do not count toward the load limit.
    ///
    /// The curve is non-monotonic: C-tier peaks at 8 (feedback loops
    /// often coordinate many components) while D-tier drops to 5
    /// (parameter decisions should have narrow scope). S-tier is
    /// tightest at 3 — paradigm decisions reference few peers.
    #[must_use]
    // Each tier is a distinct semantic knob; coincident values are
    // calibration, not a request to collapse arms.
    #[expect(
        clippy::match_same_arms,
        reason = "each tier is an independently-tunable calibration knob; coincident values are accidental, not semantic equivalence — collapsing arms would erase the calibration intent"
    )]
    pub fn max_refs(self) -> usize {
        match self {
            Self::S => 3,
            Self::A => 5,
            Self::B => 7,
            Self::C => 8,
            Self::D => 5,
        }
    }
}

/// ADR lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Draft,
    Proposed,
    Accepted,
    Rejected,
    Deprecated,
    SupersededBy(AdrId),
    /// Status line could not be parsed into a known variant.
    Invalid(String),
}

impl Status {
    /// Parse a status line. Returns `Invalid` if unrecognized.
    #[must_use]
    pub fn parse(line: &str) -> Self {
        let trimmed = line.trim();

        match trimmed {
            "Draft" => Self::Draft,
            "Proposed" => Self::Proposed,
            "Accepted" => Self::Accepted,
            "Deprecated" => Self::Deprecated,
            "Rejected" => Self::Rejected,
            s if s.starts_with("Superseded by ") => {
                let rest = &s["Superseded by ".len()..];
                match parse_adr_id(rest.trim()) {
                    Some(id) => Self::SupersededBy(id),
                    None => Self::Invalid(trimmed.to_owned()),
                }
            }
            _ => Self::Invalid(trimmed.to_owned()),
        }
    }

    /// Returns true if the raw status line has parenthetical content
    /// (e.g., `Accepted (note)`), which is not a valid status format.
    #[must_use]
    pub fn has_parenthetical(raw: &str) -> bool {
        let trimmed = raw.trim();
        // Check for `(` after the status keyword
        trimmed.contains('(') && trimmed.contains(')')
    }

    /// Returns true for terminal lifecycle states: Rejected, Deprecated,
    /// Superseded. Terminal-state ADRs must be in the stale directory
    /// and have a `## Retirement` section.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Rejected | Self::Deprecated | Self::SupersededBy(_)
        )
    }

    /// Short display string for output formatting.
    #[must_use]
    pub fn short_display(&self) -> String {
        match self {
            Self::Draft => "Draft".into(),
            Self::Proposed => "Proposed".into(),
            Self::Accepted => "Accepted".into(),
            Self::Rejected => "Rejected".into(),
            Self::Deprecated => "Deprecated".into(),
            Self::SupersededBy(id) => format!("Superseded by {id}"),
            Self::Invalid(s) => s.clone(),
        }
    }
}

/// A typed, directional relationship between two ADRs.
#[derive(Debug, Clone)]
pub struct Relationship {
    pub verb: RelVerb,
    pub target: AdrId,
    pub line: usize,
}

/// Relationship verb vocabulary.
///
/// Three permitted verbs:
/// - `References` — soft citation (citing → cited)
/// - `Supersedes` — replaces target entirely (newer → older)
/// - `Root` — self-reference marking this ADR as a tree root
///
/// Legacy verbs are retained so the parser can recognize them and
/// guidelines output can show migration paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelVerb {
    // === Permitted verbs ===
    References,
    Supersedes,
    Root,

    // === Legacy verbs (parsed for recognition; no lint rule) ===
    DependsOn,
    Extends,
    Illustrates,
    ContrastsWith,
    ScopedBy,

    // === Legacy reverse verbs (parsed for recognition; no lint rule) ===
    Informs,
    ExtendedBy,
    IllustratedBy,
    ReferencedBy,
    SupersededBy,
    Scopes,
}

impl RelVerb {
    /// True for legacy reverse verbs.
    #[must_use]
    pub fn is_reverse(self) -> bool {
        matches!(
            self,
            Self::Informs
                | Self::ExtendedBy
                | Self::IllustratedBy
                | Self::ReferencedBy
                | Self::SupersededBy
                | Self::Scopes
        )
    }

    /// Human-readable description of the verb's meaning.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Root => "Self-reference marking this ADR as a tree root",
            Self::References => "This ADR cites the target in context or consequences",
            Self::Supersedes => "Replaces target entirely; target becomes Deprecated/Superseded",
            _ => "Legacy verb — migrate to a permitted verb",
        }
    }

    /// Migration guidance for legacy verbs. Returns None for permitted verbs.
    #[must_use]
    pub fn migration(self) -> Option<&'static str> {
        match self {
            Self::DependsOn
            | Self::Extends
            | Self::Illustrates
            | Self::ContrastsWith
            | Self::ScopedBy => Some("use References"),
            Self::Informs
            | Self::ExtendedBy
            | Self::IllustratedBy
            | Self::ReferencedBy
            | Self::SupersededBy
            | Self::Scopes => Some("remove (reverse verb)"),
            _ => None,
        }
    }

    /// All permitted verb variants.
    #[must_use]
    pub fn permitted() -> &'static [Self] {
        &[Self::Root, Self::References, Self::Supersedes]
    }

    /// All legacy verb variants.
    #[must_use]
    pub fn legacy() -> &'static [Self] {
        &[
            Self::DependsOn,
            Self::Extends,
            Self::Illustrates,
            Self::ContrastsWith,
            Self::ScopedBy,
            Self::Informs,
            Self::ExtendedBy,
            Self::IllustratedBy,
            Self::ReferencedBy,
            Self::SupersededBy,
            Self::Scopes,
        ]
    }

    /// Parse a verb string from the `## Related` section.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "Root" => Some(Self::Root),
            "References" => Some(Self::References),
            "Supersedes" => Some(Self::Supersedes),
            "Depends on" => Some(Self::DependsOn),
            "Informs" => Some(Self::Informs),
            "Extends" => Some(Self::Extends),
            "Extended by" => Some(Self::ExtendedBy),
            "Illustrates" => Some(Self::Illustrates),
            "Illustrated by" => Some(Self::IllustratedBy),
            "Referenced by" => Some(Self::ReferencedBy),
            "Contrasts with" => Some(Self::ContrastsWith),
            "Superseded by" => Some(Self::SupersededBy),
            "Scopes" => Some(Self::Scopes),
            "Scoped by" => Some(Self::ScopedBy),
            _ => None,
        }
    }
}

impl fmt::Display for RelVerb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Root => "Root",
            Self::References => "References",
            Self::Supersedes => "Supersedes",
            Self::DependsOn => "Depends on",
            Self::Informs => "Informs",
            Self::Extends => "Extends",
            Self::ExtendedBy => "Extended by",
            Self::Illustrates => "Illustrates",
            Self::IllustratedBy => "Illustrated by",
            Self::ReferencedBy => "Referenced by",
            Self::ContrastsWith => "Contrasts with",
            Self::SupersededBy => "Superseded by",
            Self::Scopes => "Scopes",
            Self::ScopedBy => "Scoped by",
        };
        write!(f, "{s}")
    }
}

/// Parse a strict ADR ID like `CHE-0042`.
///
/// Accepts exactly `^[A-Z]{2,4}-[0-9]{4}$` — uppercase ASCII prefix
/// of 2–4 letters, dash, exactly 4 digits, nothing else. No
/// whitespace trimming; callers must pass clean input.
///
/// Returns `None` for any deviation: lowercase, non-ASCII, wrong
/// digit count, trailing text, leading/trailing whitespace.
///
/// Use [`parse_adr_id_from_filename_stem`] when the input is an
/// ADR filename stem like `CHE-0042-slug-words`.
///
/// Implemented with byte-level checks rather than regex per
/// AFM-0006 R1 (regex is reserved for markdown structural
/// extraction; lexical token validation may use byte checks).
#[must_use]
pub fn parse_adr_id(s: &str) -> Option<AdrId> {
    let (prefix, num_str) = s.split_once('-')?;

    // Prefix: 2-4 uppercase ASCII letters.
    let prefix_len = prefix.len();
    if !(2..=4).contains(&prefix_len) || !prefix.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }

    // Number: exactly 4 ASCII digits, no trailing junk.
    if num_str.len() != 4 || !num_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u16 = num_str.parse().ok()?;

    Some(AdrId {
        prefix: prefix.to_owned(),
        number,
    })
}

/// Parse an ADR ID from a filename stem like `CHE-0042-some-slug`.
///
/// Matches `^[A-Z]{2,4}-[0-9]{4}(?:-|$)` at the start of the stem
/// and ignores everything after the trailing dash. Returns `None`
/// if the stem does not begin with a strict ID followed by either
/// a dash or end-of-string.
///
/// Specifically rejects `CHE-00012-foo` (5 digits before the dash)
/// and `CHE-0001x` (no dash separator after digits) — both of
/// which a naive prefix-match would silently accept.
///
/// The stem is the filename with `.md` already stripped by the
/// caller. Whitespace is not trimmed.
///
/// See AFM-0006 R1 for the byte-level validation rationale.
#[must_use]
pub fn parse_adr_id_from_filename_stem(stem: &str) -> Option<AdrId> {
    let (prefix, rest) = stem.split_once('-')?;

    let prefix_len = prefix.len();
    if !(2..=4).contains(&prefix_len) || !prefix.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }

    // Take exactly 4 leading ASCII digits; reject if fewer.
    let rest_bytes = rest.as_bytes();
    if rest_bytes.len() < 4 || !rest_bytes[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }

    // Boundary: byte after the 4 digits must be `-` or end-of-string.
    // Rejects "CHE-00012-foo" (digit at position 4) and "CHE-0001x"
    // (non-dash separator).
    if rest_bytes.len() > 4 && rest_bytes[4] != b'-' {
        return None;
    }

    let number: u16 = rest[..4].parse().ok()?;

    Some(AdrId {
        prefix: prefix.to_owned(),
        number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_adr_id_strict() {
        let id = parse_adr_id("CHE-0042").unwrap();
        assert_eq!(id.prefix, "CHE");
        assert_eq!(id.number, 42);
        assert_eq!(id.to_string(), "CHE-0042");
    }

    #[test]
    fn parse_adr_id_accepts_2_to_4_letter_prefix() {
        assert!(parse_adr_id("AI-0001").is_some());
        assert!(parse_adr_id("CHE-0001").is_some());
        assert!(parse_adr_id("AFRM-0001").is_some());
    }

    #[test]
    fn parse_adr_id_rejects_trailing_text() {
        // Lenient predecessor accepted "CHE-0042-foo" → CHE-0042; strict refuses.
        assert!(parse_adr_id("CHE-0042-foo").is_none());
        assert!(parse_adr_id("CHE-0042 ").is_none());
    }

    #[test]
    fn parse_adr_id_rejects_whitespace() {
        assert!(parse_adr_id(" CHE-0042").is_none());
        assert!(parse_adr_id("CHE-0042\n").is_none());
    }

    #[test]
    fn parse_adr_id_rejects_non_ascii_prefix() {
        assert!(parse_adr_id("ÄDR-0001").is_none());
    }

    #[test]
    fn parse_adr_id_rejects_lowercase_prefix() {
        assert!(parse_adr_id("che-0001").is_none());
    }

    #[test]
    fn parse_adr_id_rejects_wrong_digit_count() {
        assert!(parse_adr_id("CHE-001").is_none());
        assert!(parse_adr_id("CHE-00001").is_none());
        assert!(parse_adr_id("CHE-").is_none());
    }

    #[test]
    fn parse_adr_id_rejects_short_or_long_prefix() {
        assert!(parse_adr_id("C-0001").is_none());
        assert!(parse_adr_id("ABCDE-0001").is_none());
    }

    #[test]
    fn parse_adr_id_empty_prefix_returns_none() {
        assert!(parse_adr_id("-0001").is_none());
        assert!(parse_adr_id("").is_none());
        assert!(parse_adr_id("-").is_none());
    }

    #[test]
    fn parse_adr_id_from_filename_stem_strips_slug() {
        let id = parse_adr_id_from_filename_stem("CHE-0042-some-slug-words").unwrap();
        assert_eq!(id.prefix, "CHE");
        assert_eq!(id.number, 42);
    }

    #[test]
    fn parse_adr_id_from_filename_stem_accepts_bare_id() {
        let id = parse_adr_id_from_filename_stem("CHE-0001").unwrap();
        assert_eq!(id.number, 1);
    }

    #[test]
    fn parse_adr_id_from_filename_stem_rejects_short_digits() {
        assert!(parse_adr_id_from_filename_stem("CHE-42-slug").is_none());
        assert!(parse_adr_id_from_filename_stem("CHE-001").is_none());
    }

    #[test]
    fn parse_adr_id_from_filename_stem_rejects_lowercase() {
        assert!(parse_adr_id_from_filename_stem("che-0001-slug").is_none());
    }

    #[test]
    fn parse_adr_id_from_filename_stem_rejects_non_ascii() {
        assert!(parse_adr_id_from_filename_stem("ÄDR-0001-slug").is_none());
    }

    #[test]
    fn parse_adr_id_from_filename_stem_rejects_five_digits() {
        // Naive prefix-match would accept first 4 digits as 0001 and
        // discard the rest. Strict boundary rejects.
        assert!(parse_adr_id_from_filename_stem("CHE-00012-foo").is_none());
    }

    #[test]
    fn parse_adr_id_from_filename_stem_rejects_non_dash_separator() {
        // Naive match would accept "0001" and ignore "x". Strict
        // boundary requires `-` or end-of-string after the 4 digits.
        assert!(parse_adr_id_from_filename_stem("CHE-0001x").is_none());
        assert!(parse_adr_id_from_filename_stem("CHE-0001_slug").is_none());
    }

    #[test]
    fn parse_status_superseded_with_trailing_space_returns_invalid() {
        // "Superseded by " with empty target after trim — strict
        // parse_adr_id rejects empty input, status falls through to Invalid.
        let s = Status::parse("Superseded by ");
        assert!(matches!(s, Status::Invalid(_)), "got {s:?}");
    }

    #[test]
    fn parse_status_accepted() {
        assert_eq!(Status::parse("Accepted"), Status::Accepted);
    }

    #[test]
    fn parse_status_rejected() {
        assert_eq!(Status::parse("Rejected"), Status::Rejected);
    }

    #[test]
    fn parse_status_amended_is_invalid() {
        let s = Status::parse("Amended 2026-04-25 — added fencing");
        assert!(matches!(s, Status::Invalid(_)));
    }

    #[test]
    fn parse_status_amended_bare_is_invalid() {
        let s = Status::parse("Amended");
        assert!(matches!(s, Status::Invalid(_)));
    }

    #[test]
    fn parse_status_superseded() {
        let s = Status::parse("Superseded by CHE-0099");
        match s {
            Status::SupersededBy(id) => {
                assert_eq!(id.prefix, "CHE");
                assert_eq!(id.number, 99);
            }
            other => panic!("expected SupersededBy, got {other:?}"),
        }
    }

    #[test]
    fn parse_status_invalid() {
        let s = Status::parse("Accepted (supersedes original u64 design)");
        assert!(matches!(s, Status::Invalid(_)));
    }

    #[test]
    fn has_parenthetical_detects_annotations() {
        assert!(Status::has_parenthetical("Accepted (note)"));
        assert!(!Status::has_parenthetical("Accepted"));
        assert!(!Status::has_parenthetical("Amended 2026-04-25 — note"));
    }

    #[test]
    fn root_verb_parse_and_display() {
        assert_eq!(RelVerb::parse("Root"), Some(RelVerb::Root));
        assert_eq!(RelVerb::Root.to_string(), "Root");
    }

    #[test]
    fn verb_display_roundtrip() {
        let verbs = [
            ("Root", RelVerb::Root),
            ("References", RelVerb::References),
            ("Supersedes", RelVerb::Supersedes),
            ("Depends on", RelVerb::DependsOn),
            ("Informs", RelVerb::Informs),
            ("Extends", RelVerb::Extends),
            ("Extended by", RelVerb::ExtendedBy),
            ("Illustrates", RelVerb::Illustrates),
            ("Illustrated by", RelVerb::IllustratedBy),
            ("Referenced by", RelVerb::ReferencedBy),
            ("Contrasts with", RelVerb::ContrastsWith),
            ("Superseded by", RelVerb::SupersededBy),
            ("Scopes", RelVerb::Scopes),
            ("Scoped by", RelVerb::ScopedBy),
        ];
        for (text, verb) in verbs {
            assert_eq!(RelVerb::parse(text), Some(verb), "parse({text})");
            assert_eq!(verb.to_string(), text, "display({verb:?})");
        }
    }

    #[test]
    fn tier_descriptions_non_empty() {
        for tier in Tier::all() {
            assert!(!tier.name().is_empty(), "{tier:?} name");
            assert!(!tier.description().is_empty(), "{tier:?} description");
            assert!(!tier.stability().is_empty(), "{tier:?} stability");
        }
    }

    #[test]
    fn tier_names_match_meadows_alignment() {
        assert_eq!(Tier::S.name(), "Intent");
        assert_eq!(Tier::A.name(), "Self-organization");
        assert_eq!(Tier::B.name(), "Design");
        assert_eq!(Tier::C.name(), "Feedbacks");
        assert_eq!(Tier::D.name(), "Parameters");
    }

    #[test]
    fn status_is_terminal() {
        assert!(!Status::Draft.is_terminal());
        assert!(!Status::Proposed.is_terminal());
        assert!(!Status::Accepted.is_terminal());
        assert!(Status::Rejected.is_terminal());
        assert!(Status::Deprecated.is_terminal());
        assert!(
            Status::SupersededBy(AdrId {
                prefix: "CHE".into(),
                number: 1
            })
            .is_terminal()
        );
    }

    #[test]
    fn verb_migration_for_legacy() {
        for verb in RelVerb::legacy() {
            assert!(
                verb.migration().is_some(),
                "{verb:?} should have migration guidance"
            );
        }
    }

    #[test]
    fn verb_migration_none_for_permitted() {
        for verb in RelVerb::permitted() {
            assert!(
                verb.migration().is_none(),
                "{verb:?} should not have migration guidance"
            );
        }
    }

    #[test]
    fn default_adr_record() {
        let record = AdrRecord::default();
        assert_eq!(record.id.prefix, "");
        assert_eq!(record.id.number, 0);
        assert!(record.crates.is_empty());
        assert!(record.decision_rules.is_empty());
    }

    #[test]
    fn tier_rank_ordering() {
        assert!(Tier::S.rank() < Tier::A.rank());
        assert!(Tier::A.rank() < Tier::B.rank());
        assert!(Tier::B.rank() < Tier::C.rank());
        assert!(Tier::C.rank() < Tier::D.rank());
        assert_eq!(Tier::D.rank(), 4);
    }

    #[test]
    fn tier_factor_ordering() {
        assert!(Tier::S.factor() > Tier::A.factor());
        assert!(Tier::A.factor() > Tier::B.factor());
        assert!((Tier::B.factor() - 1.0).abs() < f64::EPSILON);
        assert!(Tier::C.factor() < Tier::B.factor());
        assert!(Tier::D.factor() < Tier::C.factor());
    }

    #[test]
    fn tier_min_words_ordering() {
        assert!(Tier::S.min_words() >= Tier::A.min_words());
        assert!(Tier::A.min_words() >= Tier::B.min_words());
        assert!(Tier::B.min_words() >= Tier::C.min_words());
        assert!(Tier::C.min_words() >= Tier::D.min_words());
    }

    #[test]
    fn tier_max_refs_values() {
        assert_eq!(Tier::S.max_refs(), 3);
        assert_eq!(Tier::A.max_refs(), 5);
        assert_eq!(Tier::B.max_refs(), 7);
        assert_eq!(Tier::C.max_refs(), 8);
        assert_eq!(Tier::D.max_refs(), 5);
    }

    #[test]
    fn status_short_display() {
        assert_eq!(Status::Draft.short_display(), "Draft");
        assert_eq!(Status::Accepted.short_display(), "Accepted");
        assert_eq!(
            Status::SupersededBy(AdrId {
                prefix: "CHE".into(),
                number: 99,
            })
            .short_display(),
            "Superseded by CHE-0099"
        );
    }

    #[test]
    fn layer_to_tier_mapping() {
        use super::layer_to_tier;

        // S-tier: layers 1-3
        assert_eq!(layer_to_tier(1), Some(Tier::S));
        assert_eq!(layer_to_tier(2), Some(Tier::S));
        assert_eq!(layer_to_tier(3), Some(Tier::S));

        // A-tier: layer 4
        assert_eq!(layer_to_tier(4), Some(Tier::A));

        // B-tier: layers 5-6
        assert_eq!(layer_to_tier(5), Some(Tier::B));
        assert_eq!(layer_to_tier(6), Some(Tier::B));

        // C-tier: layers 7-8
        assert_eq!(layer_to_tier(7), Some(Tier::C));
        assert_eq!(layer_to_tier(8), Some(Tier::C));

        // D-tier: layers 9-12
        assert_eq!(layer_to_tier(9), Some(Tier::D));
        assert_eq!(layer_to_tier(10), Some(Tier::D));
        assert_eq!(layer_to_tier(11), Some(Tier::D));
        assert_eq!(layer_to_tier(12), Some(Tier::D));
    }

    #[test]
    fn layer_to_tier_invalid() {
        use super::layer_to_tier;

        assert_eq!(layer_to_tier(0), None);
        assert_eq!(layer_to_tier(13), None);
        assert_eq!(layer_to_tier(255), None);
    }
}
