fn sanitize_git_sha(raw: &str) -> &str {
    let candidate = raw.trim();
    let provisioned = (7..=40).contains(&candidate.len())
        && candidate.bytes().all(|b| b.is_ascii_hexdigit());
    if provisioned { candidate } else { "" }
}
