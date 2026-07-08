use std::path::Path;

use crate::Error;

pub(crate) fn write_note(path: &Path, section_id: &str, body: &str) -> Result<(), Error> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(Error::ReadNote {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let merged = merge_region(existing.as_deref(), section_id, body);
    std::fs::write(path, merged).map_err(|source| Error::WriteNote {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn begin_marker(section_id: &str) -> String {
    format!("%% architect:begin {section_id} %%")
}

pub(crate) fn end_marker(section_id: &str) -> String {
    format!("%% architect:end {section_id} %%")
}

pub(crate) fn merge_region(existing: Option<&str>, section_id: &str, body: &str) -> String {
    let fenced = fenced_block(section_id, body);
    let Some(text) = existing else {
        return fenced;
    };
    let begin = begin_marker(section_id);
    let end = end_marker(section_id);
    match split_on_region(text, &begin, &end) {
        Some((before, after)) => format!("{before}{fenced}{after}"),
        None => append_block(text, &fenced),
    }
}

fn fenced_block(section_id: &str, body: &str) -> String {
    let mut fenced = begin_marker(section_id);
    fenced.push('\n');
    fenced.push_str(body);
    if !body.ends_with('\n') {
        fenced.push('\n');
    }
    fenced.push_str(&end_marker(section_id));
    fenced.push('\n');
    fenced
}

fn append_block(existing: &str, fenced: &str) -> String {
    if existing.is_empty() {
        return fenced.to_string();
    }
    let mut combined = existing.to_string();
    if !combined.ends_with('\n') {
        combined.push('\n');
    }
    combined.push('\n');
    combined.push_str(fenced);
    combined
}

fn find_line_anchored(text: &str, pattern: &str, from: usize) -> Option<usize> {
    text[from..].match_indices(pattern).find_map(|(rel, _)| {
        let idx = from + rel;
        (idx == 0 || text.as_bytes()[idx - 1] == b'\n').then_some(idx)
    })
}

fn split_on_region<'a>(text: &'a str, begin: &str, end: &str) -> Option<(&'a str, &'a str)> {
    let begin_idx = find_line_anchored(text, begin, 0)?;
    let after_begin = begin_idx + begin.len();
    let end_idx = find_line_anchored(text, end, after_begin)?;
    if let Some(next_begin_idx) = find_line_anchored(text, begin, after_begin)
        && next_begin_idx < end_idx
    {
        return None;
    }
    let after_end = end_idx + end.len();
    let after_end = text[after_end..]
        .find('\n')
        .map_or(text.len(), |nl| after_end + nl + 1);
    Some((&text[..begin_idx], &text[after_end..]))
}

#[cfg(test)]
mod tests {
    use super::{begin_marker, end_marker, merge_region};

    #[test]
    fn first_generation_has_no_outside_content() {
        let out = merge_region(None, "alpha", "Alpha crate.");
        assert_eq!(
            out,
            format!(
                "{}\nAlpha crate.\n{}\n",
                begin_marker("alpha"),
                end_marker("alpha")
            )
        );
    }

    #[test]
    fn regeneration_preserves_content_outside_the_fence() {
        let first = merge_region(None, "alpha", "Alpha crate v1.");
        let hand_edited = format!("# My notes\n\n{first}\nFooter kept as-is.\n");

        let second = merge_region(Some(&hand_edited), "alpha", "Alpha crate v2.");

        assert!(second.starts_with("# My notes\n\n"));
        assert!(second.ends_with("Footer kept as-is.\n"));
        assert!(second.contains("Alpha crate v2."));
        assert!(!second.contains("Alpha crate v1."));
    }

    #[test]
    fn identical_regeneration_is_byte_stable() {
        let hand_edited = format!(
            "Header.\n\n{}\n",
            merge_region(None, "alpha", "Alpha crate.")
        );

        let first = merge_region(Some(&hand_edited), "alpha", "Alpha crate.");
        let second = merge_region(Some(&first), "alpha", "Alpha crate.");

        assert_eq!(first, second);
    }

    #[test]
    fn missing_fence_appends_rather_than_discarding_existing_content() {
        let existing = "Hand-written note with no fence yet.\n";
        let out = merge_region(Some(existing), "alpha", "Alpha crate.");

        assert!(out.starts_with(existing));
        assert!(out.contains(&begin_marker("alpha")));
        assert!(out.contains("Alpha crate."));
    }

    #[test]
    fn second_regeneration_after_corrupted_end_marker_does_not_delete_prior_content() {
        let first = merge_region(None, "alpha", "Alpha crate v1.");
        let end = end_marker("alpha");
        let corrupted = first.replace(&format!("{end}\n"), "");
        let hand_edited = format!("{corrupted}\nHand note after corruption.\n");

        let regen1 = merge_region(Some(&hand_edited), "alpha", "Alpha crate v2.");
        assert!(
            regen1.contains("Hand note after corruption."),
            "first post-corruption regen must preserve the hand note: {regen1}"
        );

        let regen2 = merge_region(Some(&regen1), "alpha", "Alpha crate v3.");
        assert!(
            regen2.contains("Hand note after corruption."),
            "second post-corruption regen must not silently delete the hand note: {regen2}"
        );
        assert!(
            regen2.contains("Alpha crate v2."),
            "second post-corruption regen must not delete run 2's own generated block: {regen2}"
        );
    }

    #[test]
    fn marker_lookalike_text_mid_line_is_not_mistaken_for_a_real_boundary() {
        let existing = format!(
            "See syntax like {} for reference.\n{}\nBody v1.\n{}\n",
            begin_marker("alpha"),
            begin_marker("alpha"),
            end_marker("alpha")
        );

        let out = merge_region(Some(&existing), "alpha", "Body v2.");

        assert!(
            out.contains("for reference."),
            "prose line carrying lookalike marker text must survive: {out}"
        );
        assert!(out.contains("Body v2."));
        assert!(!out.contains("Body v1."));
    }

    #[test]
    fn fenced_block_is_lf_only_even_when_existing_content_is_crlf() {
        let existing = "Header line.\r\n\r\n";
        let out = merge_region(Some(existing), "alpha", "Body text.");

        assert!(out.starts_with(existing));
        let inserted = &out[existing.len()..];
        assert!(
            !inserted.contains('\r'),
            "inserted fenced block must stay LF-only: {inserted:?}"
        );
        assert!(inserted.contains(&begin_marker("alpha")));
    }
}
