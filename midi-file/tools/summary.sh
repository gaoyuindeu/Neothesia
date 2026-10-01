#!/usr/bin/env bash
# Totals of score reader result files: recall, precision, why fingers were lost
for f in "$@"; do
    awk -v f="$f" '{
        for (i = 1; i <= NF; i++) {
            if ($i == "fingered") fing += $(i+1); if ($i == "correct") cor += $(i+1)
            if ($i == "wrong") wr += $(i+1); if ($i == "extra") ex += $(i+1)
            if ($i == "unaligned") un += $(i+1); if ($i == "digit" && $(i-1) == "no") nd += $(i+1)
            if ($i == "digit" && $(i-1) == "other") od += $(i+1); if ($i == "excluded") exc += $(i+1)
        }
        n++
    } END {
        printf "%-28s pieces %d fingered %d correct %d recall %.1f%% precision %.1f%% | wrong %d extra %d | lost: unaligned %d no digit %d other digit %d | excluded notes %d\n",
            f, n, fing, cor, 100*cor/fing, 100*cor/(cor+wr+ex), wr, ex, un, nd, od, exc
    }' "$f"
done
