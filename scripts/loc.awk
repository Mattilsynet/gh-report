#!/usr/bin/env awk -f
# Per-file Rust LOC classifier.
# Emits one TSV line per file:
#   <crate>\t<path>\t<blank>\t<code>\t<comment>\t<test>\t<total>
#
# Categories (mutually exclusive on each line for blank/code/comment):
#   blank   - whitespace only
#   code    - any non-comment token; trailing // stays code
#   comment - line is only //, ///, //! or fully inside /* */
# test is an OVERLAY column (already counted in one of the three above).
#
# Test attribution:
#   - whole file if path matches /tests/, /benches/, /examples/
#   - inside src/: lines within an item armed by #[cfg(test)] or #[test]
#     (brace-depth tracker; arms next mod/fn/impl item; disarms at matching })
#
# Heuristic: not a real lexer. // or /* inside string literals would miscount.
# /* */ nesting is tracked. Trailing comments on code lines: line is code.

function classify_line(line,    s, c1, has_code, has_cmt) {
    # Returns category code for this line, updates block_depth.
    # 0=blank 1=code 2=comment
    s = line
    sub(/^[ \t]+/, "", s)
    sub(/[ \t]+$/, "", s)
    if (s == "" && block_depth == 0) return 0

    has_code = 0
    has_cmt  = 0

    # If we start inside a block comment, consume until close (or EOL).
    if (block_depth > 0) {
        has_cmt = 1
        while (block_depth > 0 && length(s) > 0) {
            c1 = substr(s, 1, 2)
            if (c1 == "*/") {
                block_depth--
                s = substr(s, 3)
            } else if (c1 == "/*") {
                block_depth++
                s = substr(s, 3)
            } else {
                s = substr(s, 2)
            }
        }
        # After closing, fall through to scan remainder of s.
    }

    # Scan remainder for code/comment tokens, respecting string/char literals.
    while (length(s) > 0) {
        c1 = substr(s, 1, 2)
        ch = substr(s, 1, 1)
        if (block_depth > 0) {
            # inside block: scan for close
            while (block_depth > 0 && length(s) > 0) {
                c1 = substr(s, 1, 2)
                if (c1 == "*/") { block_depth--; s = substr(s, 3) }
                else if (c1 == "/*") { block_depth++; s = substr(s, 3) }
                else { s = substr(s, 2) }
            }
            continue
        }
        if (c1 == "//") {
            has_cmt = 1
            s = ""
            break
        }
        if (c1 == "/*") {
            has_cmt = 1
            block_depth++
            s = substr(s, 3)
            continue
        }
        if (ch == "\"") {
            # consume string literal
            s = substr(s, 2)
            while (length(s) > 0) {
                ch = substr(s, 1, 1)
                if (ch == "\\") { s = substr(s, 3); continue }
                if (ch == "\"") { s = substr(s, 2); break }
                s = substr(s, 2)
            }
            has_code = 1
            continue
        }
        if (ch == "'") {
            # char literal or lifetime — just consume one char and move on
            has_code = 1
            s = substr(s, 2)
            continue
        }
        if (ch ~ /[ \t]/) {
            s = substr(s, 2)
            continue
        }
        has_code = 1
        s = substr(s, 2)
    }

    if (has_code) return 1
    if (has_cmt)  return 2
    return 0
}

function reset_file() {
    block_depth = 0
    brace_depth = 0
    test_armed_depth = -1   # if >=0, we are inside a test-armed item; disarm when brace_depth drops below this
    pending_test_arm = 0     # next item-opener arms at its opening brace
    file_test_mode = 0
    blank=0; code=0; cmt=0; test=0; total=0
}

function update_test_state(line,    s, open_braces, close_braces, i, ch, n) {
    # Crude brace tracker, ignores braces in strings/comments only loosely.
    # We approximate by counting { and } in the line; given Rust style this is fine for cfg(test) gating.
    n = length(line)
    for (i = 1; i <= n; i++) {
        ch = substr(line, i, 1)
        if (ch == "{") {
            brace_depth++
            if (pending_test_arm) {
                test_armed_depth = brace_depth
                pending_test_arm = 0
            }
        } else if (ch == "}") {
            if (test_armed_depth >= 0 && brace_depth == test_armed_depth) {
                test_armed_depth = -1
            }
            brace_depth--
            if (brace_depth < 0) brace_depth = 0
        }
    }
    # arm on attribute lines
    if (line ~ /#\[cfg\(test\)\]/ || line ~ /#\[cfg\(any\([^)]*test/ || line ~ /#\[test\]/ || line ~ /#\[tokio::test\]/) {
        pending_test_arm = 1
    }
}

BEGIN {
    OFS = "\t"
}

FNR == 1 {
    if (NR > 1) emit_file()
    reset_file()
    current_file = FILENAME
    # determine file_test_mode by path
    if (current_file ~ /\/tests\// || current_file ~ /\/benches\// || current_file ~ /\/examples\//) {
        file_test_mode = 1
    } else {
        file_test_mode = 0
    }
    # crate name = path component after "crates/"
    n = split(current_file, parts, "/")
    for (i = 1; i <= n; i++) {
        if (parts[i] == "crates" && i+1 <= n) { current_crate = parts[i+1]; break }
    }
}

{
    cat = classify_line($0)
    total++
    if (cat == 0) blank++
    else if (cat == 1) code++
    else cmt++

    is_test = file_test_mode || (test_armed_depth >= 0)
    if (is_test) test++

    update_test_state($0)
}

function emit_file() {
    print current_crate, current_file, blank, code, cmt, test, total
}

END {
    if (current_file != "") emit_file()
}
