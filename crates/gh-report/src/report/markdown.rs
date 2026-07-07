//! Bounded markdown renderer for `OPERATIONS.md` (UF3-2).
//!
//! This is not a general `CommonMark` implementation. It renders exactly the
//! constructs present in a single closed, repo-owned document: ATX headings
//! (with GitHub-style slug `id=` anchors), fenced code blocks, GFM pipe
//! tables (no column alignment), flat ordered/unordered lists, blockquotes,
//! paragraphs, and the inline subset (code spans, bold, italic, links,
//! backslash escapes). COM-0016:R1 is cleared by direct inspection of the
//! source document rather than by generalizing to arbitrary markdown.

/// Render a markdown document to an HTML fragment.
///
/// Every ATX heading receives a GitHub-style slug `id=` attribute (see
/// [`slugify_heading`]), so `href="#slug"` deep links resolve.
///
/// # Examples
///
/// ```
/// use gh_report::report::markdown::render;
///
/// let html = render("## Branch Protection Coverage\n\nSome text.\n");
/// assert!(html.contains(r#"<h2 id="branch-protection-coverage">Branch Protection Coverage</h2>"#));
/// ```
#[must_use]
pub fn render(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::with_capacity(source.len() * 2);
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some(lang) = fence_lang(line) {
            let code_start = i + 1;
            let mut end = code_start;
            while end < lines.len() && !is_fence_line(lines[end]) {
                end += 1;
            }
            render_code_block(&mut out, lang, &lines[code_start..end]);
            i = (end + 1).min(lines.len());
            continue;
        }
        if let Some((level, text)) = parse_heading(line) {
            render_heading(&mut out, level, text);
            i += 1;
            continue;
        }
        if is_table_start(&lines, i) {
            i = render_table(&mut out, &lines, i);
            continue;
        }
        if strip_blockquote_marker(line).is_some() {
            i = render_blockquote(&mut out, &lines, i);
            continue;
        }
        if strip_unordered_marker(line).is_some() {
            i = render_list(&mut out, &lines, i, ListKind::Unordered);
            continue;
        }
        if strip_ordered_marker(line).is_some() {
            i = render_list(&mut out, &lines, i, ListKind::Ordered);
            continue;
        }
        i = render_paragraph(&mut out, &lines, i);
    }
    out
}

/// Compute a GitHub-style heading slug: lowercase, ASCII spaces become
/// hyphens, everything outside `[a-z0-9-]` is dropped.
///
/// # Examples
///
/// ```
/// use gh_report::report::markdown::slugify_heading;
///
/// assert_eq!(slugify_heading("Branch Protection Coverage"), "branch-protection-coverage");
/// assert_eq!(
///     slugify_heading("Kubernetes / Knative Probe Configuration"),
///     "kubernetes--knative-probe-configuration"
/// );
/// ```
#[must_use]
pub fn slugify_heading(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_whitespace() {
            slug.push('-');
        } else if ch.is_ascii_alphanumeric() || ch == '-' {
            slug.extend(ch.to_lowercase());
        }
    }
    slug
}

#[derive(Clone, Copy)]
enum ListKind {
    Ordered,
    Unordered,
}

fn fence_lang(line: &str) -> Option<&str> {
    line.strip_prefix("```").map(str::trim)
}

fn is_fence_line(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    let text = rest.strip_prefix(' ')?;
    let text = text.trim_end().trim_end_matches('#').trim_end();
    Some((u8::try_from(hashes).unwrap_or(6), text))
}

fn strip_unordered_marker(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix("- ")
}

fn strip_ordered_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let digits_len = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits_len == 0 {
        return None;
    }
    trimmed[digits_len..].strip_prefix(". ")
}

fn strip_blockquote_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start().strip_prefix('>')?;
    Some(trimmed.strip_prefix(' ').unwrap_or(trimmed))
}

fn is_table_separator_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn is_table_start(lines: &[&str], i: usize) -> bool {
    lines[i].contains('|')
        && lines
            .get(i + 1)
            .is_some_and(|next| is_table_separator_row(next))
}

fn split_table_row(line: &str) -> Vec<&str> {
    let trimmed = line.trim();
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    trimmed.split('|').map(str::trim).collect()
}

fn render_heading(out: &mut String, level: u8, text: &str) {
    let slug = slugify_heading(text);
    out.push_str("<h");
    out.push((b'0' + level) as char);
    out.push_str(" id=\"");
    out.push_str(&slug);
    out.push_str("\">");
    render_inline(text, out);
    out.push_str("</h");
    out.push((b'0' + level) as char);
    out.push_str(">\n");
}

fn render_code_block(out: &mut String, lang: &str, lines: &[&str]) {
    out.push_str("<pre><code");
    if !lang.is_empty() {
        out.push_str(" class=\"language-");
        push_escaped_str(out, lang);
        out.push('"');
    }
    out.push('>');
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        push_escaped_str(out, line);
    }
    out.push_str("</code></pre>\n");
}

fn render_table(out: &mut String, lines: &[&str], start: usize) -> usize {
    let header_cells = split_table_row(lines[start]);
    out.push_str("<table class=\"data-table\">\n<thead>\n<tr>");
    for cell in &header_cells {
        out.push_str("<th>");
        render_inline(cell, out);
        out.push_str("</th>");
    }
    out.push_str("</tr>\n</thead>\n<tbody>\n");

    let mut i = start + 2;
    while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
        out.push_str("<tr>");
        for cell in split_table_row(lines[i]) {
            out.push_str("<td>");
            render_inline(cell, out);
            out.push_str("</td>");
        }
        out.push_str("</tr>\n");
        i += 1;
    }
    out.push_str("</tbody>\n</table>\n");
    i
}

fn render_list(out: &mut String, lines: &[&str], start: usize, kind: ListKind) -> usize {
    let strip = match kind {
        ListKind::Ordered => strip_ordered_marker,
        ListKind::Unordered => strip_unordered_marker,
    };
    let tag = match kind {
        ListKind::Ordered => "ol",
        ListKind::Unordered => "ul",
    };
    out.push('<');
    out.push_str(tag);
    out.push_str(">\n");

    let mut i = start;
    while i < lines.len() {
        let Some(item) = strip(lines[i]) else { break };
        out.push_str("<li>");
        render_inline(item.trim(), out);
        out.push_str("</li>\n");
        i += 1;
    }

    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
    i
}

fn render_blockquote(out: &mut String, lines: &[&str], start: usize) -> usize {
    let mut content = String::new();
    let mut i = start;
    while i < lines.len() {
        let Some(rest) = strip_blockquote_marker(lines[i]) else {
            break;
        };
        if !content.is_empty() {
            content.push(' ');
        }
        content.push_str(rest.trim());
        i += 1;
    }
    out.push_str("<blockquote><p>");
    render_inline(&content, out);
    out.push_str("</p></blockquote>\n");
    i
}

fn is_block_boundary(lines: &[&str], i: usize) -> bool {
    let line = lines[i];
    line.trim().is_empty()
        || fence_lang(line).is_some()
        || parse_heading(line).is_some()
        || is_table_start(lines, i)
        || strip_blockquote_marker(line).is_some()
        || strip_unordered_marker(line).is_some()
        || strip_ordered_marker(line).is_some()
}

fn render_paragraph(out: &mut String, lines: &[&str], start: usize) -> usize {
    let mut content = String::new();
    let mut i = start;
    while i < lines.len() && (i == start || !is_block_boundary(lines, i)) {
        if !content.is_empty() {
            content.push(' ');
        }
        content.push_str(lines[i].trim());
        i += 1;
    }
    out.push_str("<p>");
    render_inline(&content, out);
    out.push_str("</p>\n");
    i
}

fn render_inline(text: &str, out: &mut String) {
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        match c {
            '\\' => {
                let mut chars = rest.chars();
                chars.next();
                match chars.next() {
                    Some(next) if next.is_ascii_punctuation() => {
                        push_escaped_char(out, next);
                        rest = &rest[1 + next.len_utf8()..];
                    }
                    _ => {
                        out.push('\\');
                        rest = &rest[1..];
                    }
                }
            }
            '`' => {
                if let Some(end) = rest[1..].find('`') {
                    out.push_str("<code>");
                    push_escaped_str(out, &rest[1..=end]);
                    out.push_str("</code>");
                    rest = &rest[1 + end + 1..];
                } else {
                    push_escaped_char(out, c);
                    rest = &rest[1..];
                }
            }
            '*' if rest.starts_with("**") => {
                if let Some(end) = rest[2..].find("**") {
                    out.push_str("<strong>");
                    render_inline(&rest[2..2 + end], out);
                    out.push_str("</strong>");
                    rest = &rest[2 + end + 2..];
                } else {
                    push_escaped_char(out, c);
                    rest = &rest[1..];
                }
            }
            '*' => {
                if let Some(end) = rest[1..].find('*') {
                    out.push_str("<em>");
                    render_inline(&rest[1..=end], out);
                    out.push_str("</em>");
                    rest = &rest[1 + end + 1..];
                } else {
                    push_escaped_char(out, c);
                    rest = &rest[1..];
                }
            }
            '[' => {
                if let Some(link_html_and_rest) = try_render_link(rest) {
                    let (html, remainder) = link_html_and_rest;
                    out.push_str(&html);
                    rest = remainder;
                } else {
                    push_escaped_char(out, c);
                    rest = &rest[1..];
                }
            }
            _ => {
                push_escaped_char(out, c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
}

fn try_render_link(rest: &str) -> Option<(String, &str)> {
    let after_open = &rest[1..];
    let close_bracket = after_open.find(']')?;
    let link_text = &after_open[..close_bracket];
    let after_bracket = &after_open[close_bracket + 1..];
    let after_paren_open = after_bracket.strip_prefix('(')?;
    let close_paren = after_paren_open.find(')')?;
    let url = &after_paren_open[..close_paren];

    let mut html = String::from("<a href=\"");
    push_escaped_str(&mut html, url);
    html.push_str("\">");
    render_inline(link_text, &mut html);
    html.push_str("</a>");

    Some((html, &after_paren_open[close_paren + 1..]))
}

fn push_escaped_char(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        _ => out.push(c),
    }
}

fn push_escaped_str(out: &mut String, s: &str) {
    for c in s.chars() {
        push_escaped_char(out, c);
    }
}

#[cfg(test)]
mod tests {
    use super::{render, slugify_heading};

    #[test]
    fn slugify_matches_every_pre_existing_anchor_in_the_codebase() {
        let cases = [
            ("Security Policy Coverage", "security-policy-coverage"),
            ("Dependabot Coverage", "dependabot-coverage"),
            ("Secret Scanning Coverage", "secret-scanning-coverage"),
            ("Branch Protection Coverage", "branch-protection-coverage"),
            ("CODEOWNERS Coverage", "codeowners-coverage"),
            (
                "Fine-grained PAT / GitHub App",
                "fine-grained-pat--github-app",
            ),
            ("Capability probes", "capability-probes"),
            (
                "1. GitHub App (recommended for production)",
                "1-github-app-recommended-for-production",
            ),
            (
                "Kubernetes / Knative Probe Configuration",
                "kubernetes--knative-probe-configuration",
            ),
        ];
        for (heading, expected_slug) in cases {
            assert_eq!(slugify_heading(heading), expected_slug, "{heading}");
        }
    }

    #[test]
    fn heading_renders_id_and_level() {
        let html = render("## Branch Protection Coverage\n");
        assert!(
            html.contains(r#"<h2 id="branch-protection-coverage">Branch Protection Coverage</h2>"#)
        );

        let html = render("#### Security Policy Coverage\n");
        assert!(
            html.contains(r#"<h4 id="security-policy-coverage">Security Policy Coverage</h4>"#)
        );

        let html = render("# Operations Guide\n");
        assert!(html.contains(r#"<h1 id="operations-guide">Operations Guide</h1>"#));
    }

    #[test]
    fn paragraph_renders_bold_italic_and_code_span() {
        let html = render("Some **bold** and *italic* and `code`.\n");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
        assert!(html.contains("<code>code</code>"));
        assert!(html.contains("<p>"));
        assert!(html.contains("</p>"));
    }

    #[test]
    fn backslash_escape_renders_literal_asterisk_not_italic() {
        let html = render("\\* Provide exactly one of `X`.\n");
        assert!(!html.contains("<em>"));
        assert!(html.contains("* Provide exactly one of <code>X</code>."));
    }

    #[test]
    fn code_span_escapes_html_special_characters() {
        let html = render("Uses `<meta http-equiv=\"refresh\">` for warm start.\n");
        assert!(html.contains("&lt;meta"));
        assert!(!html.contains("<meta http-equiv"));
    }

    #[test]
    fn internal_fragment_link_renders_anchor_href() {
        let html = render("See [Schema Versions](#schema-versions) for details.\n");
        assert!(html.contains(r##"<a href="#schema-versions">Schema Versions</a>"##));
    }

    #[test]
    fn cross_page_fragment_link_renders_verbatim_href() {
        let html = render("See [Metric Caveats](report.html#metric-caveats) for tiers.\n");
        assert!(html.contains(r#"<a href="report.html#metric-caveats">Metric Caveats</a>"#));
    }

    #[test]
    fn fenced_code_block_escapes_body_and_carries_language_class() {
        let source = "```sh\nexport GITHUB_TOKEN=\"a<b\"\n```\n";
        let html = render(source);
        assert!(html.contains(r#"<pre><code class="language-sh">"#));
        assert!(html.contains("export GITHUB_TOKEN=&quot;a&lt;b&quot;"));
        assert!(html.contains("</code></pre>"));
    }

    #[test]
    fn fenced_code_block_without_language_omits_class_attribute() {
        let source = "```\nstore/\n```\n";
        let html = render(source);
        assert!(html.contains("<pre><code>store/"));
    }

    #[test]
    fn fenced_code_block_preserves_blank_lines_inside() {
        let source = "```yaml\na: 1\n\nb: 2\n```\n";
        let html = render(source);
        assert!(html.contains("a: 1\n\nb: 2"));
    }

    #[test]
    fn table_renders_header_and_body_rows() {
        let source = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";
        let html = render(source);
        assert!(html.contains(r#"<table class="data-table">"#));
        assert_eq!(html.matches("<th>").count(), 2);
        assert_eq!(html.matches("<td>").count(), 4);
        assert!(html.contains("<th>A</th>"));
        assert!(html.contains("<td>1</td>"));
    }

    #[test]
    fn unordered_list_renders_one_li_per_item() {
        let source = "- first\n- second\n- third\n";
        let html = render(source);
        assert!(html.contains("<ul>"));
        assert_eq!(html.matches("<li>").count(), 3);
        assert!(html.contains("<li>first</li>"));
    }

    #[test]
    fn ordered_list_renders_ol_with_items() {
        let source = "1. first\n2. second\n";
        let html = render(source);
        assert!(html.contains("<ol>"));
        assert_eq!(html.matches("<li>").count(), 2);
        assert!(html.contains("<li>first</li>"));
    }

    #[test]
    fn multiline_blockquote_joins_into_one_paragraph() {
        let source = "> line one\n> line two\n";
        let html = render(source);
        assert_eq!(html.matches("<blockquote>").count(), 1);
        assert!(html.contains("line one line two"));
    }

    #[test]
    fn paragraph_soft_breaks_join_with_space() {
        let source = "line one\nline two\n";
        let html = render(source);
        assert!(html.contains("<p>line one line two</p>"));
    }

    #[test]
    fn blank_line_separates_paragraphs() {
        let source = "para one\n\npara two\n";
        let html = render(source);
        assert_eq!(html.matches("<p>").count(), 2);
    }

    #[test]
    fn full_operations_document_carries_all_required_anchors() {
        let source = include_str!("../../OPERATIONS.md");
        let html = render(source);
        for anchor in [
            "security-policy-coverage",
            "dependabot-coverage",
            "secret-scanning-coverage",
            "branch-protection-coverage",
            "codeowners-coverage",
            "fine-grained-pat--github-app",
            "capability-probes",
            "1-github-app-recommended-for-production",
            "kubernetes--knative-probe-configuration",
        ] {
            let needle = format!(r#"id="{anchor}""#);
            assert!(html.contains(&needle), "missing anchor: {anchor}");
        }
    }

    #[test]
    fn full_operations_document_renders_without_leaking_fence_markers() {
        let source = include_str!("../../OPERATIONS.md");
        let html = render(source);
        assert!(
            !html.contains("```"),
            "fenced code markers must not leak into HTML output"
        );
    }
}
