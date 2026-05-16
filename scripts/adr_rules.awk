#!/usr/bin/env awk -f
# Per-ADR rule counter.
# Counts top-level bullet items inside ## Decision or ## Rules sections.
# Emits TSV: <domain>\t<file>\t<rule_count>\t<has_decision_or_rules_section>

BEGIN { FS = ""; OFS = "\t" }

FNR == 1 {
    if (NR > 1) emit()
    file = FILENAME
    rules = 0
    has_section = 0
    in_section = 0
    # domain = directory under docs/adr/
    n = split(file, p, "/")
    domain = ""
    for (i = 1; i <= n; i++) {
        if (p[i] == "adr" && i+1 <= n) { domain = p[i+1]; break }
    }
}

{
    line = $0
    # detect headings
    if (line ~ /^##[ \t]+/) {
        # strip ## and whitespace
        h = line
        sub(/^##[ \t]+/, "", h)
        sub(/[ \t]+$/, "", h)
        # lower-case compare
        hl = tolower(h)
        if (hl == "decision" || hl == "rules" || hl ~ /^decision[: ]/ || hl ~ /^rules[: ]/) {
            in_section = 1
            has_section = 1
        } else {
            in_section = 0
        }
        next
    }
    # heading at any other level resets section if it's H1/H2
    if (line ~ /^#[ \t]+/) { in_section = 0; next }

    if (in_section) {
        # Idiomatic ADR rule line: ^R<digits> [optional priority]: text
        if (line ~ /^R[0-9]+([ \t]*\[[^]]+\])?[ \t]*:/) rules++
    }
}

function emit() {
    print domain, file, rules, has_section
}

END { if (file != "") emit() }
