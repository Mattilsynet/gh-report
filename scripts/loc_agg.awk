#!/usr/bin/env awk -f
# Aggregate per-file TSV from loc.awk into per-crate and per-family tables.
# Input columns: crate path blank code cmt test total

BEGIN { FS = "\t"; OFS = "\t" }

{
    crate = $1
    files[crate]++
    blank[crate] += $3
    code[crate]  += $4
    cmt[crate]   += $5
    test[crate]  += $6
    total[crate] += $7
}

function family_of(c) {
    if (c == "adr-fmt") return "adr-fmt"
    if (c == "gh-report") return "gh-report"
    if (c ~ /^cherry-pit-/) return "cherry-pit-*"
    if (c ~ /^pardosa/) return "pardosa*"
    return "other"
}

END {
    # collect sorted crate list
    n = 0
    for (c in files) crates[++n] = c
    # simple insertion sort
    for (i = 2; i <= n; i++) {
        v = crates[i]; j = i - 1
        while (j > 0 && crates[j] > v) { crates[j+1] = crates[j]; j-- }
        crates[j+1] = v
    }

    printf "%-22s %6s %7s %8s %9s %7s %8s\n", "crate", "files", "blank", "code", "comment", "test*", "total"
    printf "%-22s %6s %7s %8s %9s %7s %8s\n", "----------------------", "------", "-------", "--------", "---------", "-------", "--------"

    # per-crate, grouped by family
    fam_order[1] = "adr-fmt"
    fam_order[2] = "cherry-pit-*"
    fam_order[3] = "pardosa*"
    fam_order[4] = "gh-report"

    for (fi = 1; fi <= 4; fi++) {
        fam = fam_order[fi]
        fam_files = 0; fam_blank = 0; fam_code = 0; fam_cmt = 0; fam_test = 0; fam_total = 0
        for (i = 1; i <= n; i++) {
            c = crates[i]
            if (family_of(c) != fam) continue
            printf "%-22s %6d %7d %8d %9d %7d %8d\n", c, files[c], blank[c], code[c], cmt[c], test[c], total[c]
            fam_files += files[c]; fam_blank += blank[c]; fam_code += code[c]
            fam_cmt += cmt[c]; fam_test += test[c]; fam_total += total[c]
        }
        printf "%-22s %6d %7d %8d %9d %7d %8d\n", "  = " fam, fam_files, fam_blank, fam_code, fam_cmt, fam_test, fam_total
        printf "\n"

        ws_files += fam_files; ws_blank += fam_blank; ws_code += fam_code
        ws_cmt += fam_cmt; ws_test += fam_test; ws_total += fam_total
    }

    printf "%-22s %6d %7d %8d %9d %7d %8d\n", "WORKSPACE TOTAL", ws_files, ws_blank, ws_code, ws_cmt, ws_test, ws_total
}
