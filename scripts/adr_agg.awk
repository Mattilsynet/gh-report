#!/usr/bin/env awk -f
# Aggregate ADR rule TSV.
# Input cols: domain file rules has_section
BEGIN { FS = "\t" }

{
    d = $1
    docs[d]++
    rules[d] += $3
    if ($3 == 0 && $4 == 1) zero_with_section[d]++
    if ($4 == 0) no_section[d]++
    # remember top files
    key = d "\t" $3 "\t" $2
    all[NR] = d "\t" $3 "\t" $2
}

END {
    fmt = "%-12s %6s %7s %12s %10s %10s\n"
    printf fmt, "domain", "ADRs", "rules", "rules/ADR", "zero(sec)", "no-sec"
    printf fmt, "------------", "------", "-------", "------------", "----------", "----------"

    # sorted domain list
    n = 0
    for (d in docs) doms[++n] = d
    for (i = 2; i <= n; i++) {
        v = doms[i]; j = i - 1
        while (j > 0 && doms[j] > v) { doms[j+1] = doms[j]; j-- }
        doms[j+1] = v
    }

    tot_docs = 0; tot_rules = 0; tot_zws = 0; tot_nosec = 0
    for (i = 1; i <= n; i++) {
        d = doms[i]
        avg = docs[d] > 0 ? sprintf("%.2f", rules[d]/docs[d]) : "-"
        printf "%-12s %6d %7d %12s %10d %10d\n", d, docs[d], rules[d], avg, zero_with_section[d]+0, no_section[d]+0
        tot_docs += docs[d]; tot_rules += rules[d]
        tot_zws += zero_with_section[d]+0; tot_nosec += no_section[d]+0
    }
    avg = tot_docs > 0 ? sprintf("%.2f", tot_rules/tot_docs) : "-"
    printf "%-12s %6s %7s %12s %10s %10s\n", "------------", "------", "-------", "------------", "----------", "----------"
    printf "%-12s %6d %7d %12s %10d %10d\n", "TOTAL", tot_docs, tot_rules, avg, tot_zws, tot_nosec
}
