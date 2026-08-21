#!/bin/sh
# The port's ledger, asserted where its ladder cannot run.
#
# WHY THIS EXISTS, because a check that replaces a check has to say what it
# kept. `nplan port check --wave=4` walks four waves — markup, translate,
# tests, idiomatize — and each is gated per symbol. Ronin was ported from C
# samurai, and that C corpus has been retired from this repository, so the
# markup wave's second half (an annotation in the source file the symbol came
# from) can never be satisfied: `build.c` and its neighbours are not here and
# will not be again. The waves are cumulative, so with markup unsatisfiable
# the whole ladder reads 0/170 and refuses, whatever the other columns say.
#
# That is the part worth being precise about. A gate that is red for a reason
# nothing can change does not verify anything: it cannot tell a lost claim from
# a retired source, and `check-release.sh` is `set -eu`, so for as long as it
# sat in the middle of that script nothing below it ran at all. What the ladder
# WOULD have been verifying, if it could run, is still verifiable — three of
# `nplan port status`'s four columns have subjects that exist — and that is
# exactly what this script asserts, per symbol, failing closed:
#
#   R  the def/sem rules for the symbol are written under docs/spec/port/
#   T  Rust code claims those rules
#   V  a test claims those rules
#
# And the fourth column is asserted EMPTY rather than ignored, which is the
# only honest way to record a retirement: if a source annotation ever comes
# back, the C corpus came back with it, and the ladder — not this script — is
# what should be gating the port again.
#
# `nplan spec uncovered` does not cover this. It tracks `req` and `def` rules;
# the 151 `sem` rules are outside it, and they are the ones that say what each
# ported function actually does. Measured rather than assumed: removing one
# `[spec:ronin:sem:build.buildreset-fn]` annotation from src/build.rs leaves
# `nplan spec uncovered` reporting no uncovered rules, and turns this script
# red.
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# The manifest is frozen: its source was retired, so the number cannot grow by
# porting and must not shrink by accident. A change here is a change to what
# the port claims to have covered, and it should arrive as an edit to this line
# with a reason beside it.
expected_symbols=170

status=$(nplan port status --color never)
printf '%s\n' "$status"

printf '%s\n' "$status" | awk -v expected="$expected_symbols" '
    # A matrix row is a symbol, its kind, and the four marks. The header line
    # has the same field count and is told apart by its marks not being marks.
    NF == 6 && $3 ~ /^(✓|·)$/ && $4 ~ /^(✓|·)$/ && $5 ~ /^(✓|·)$/ && $6 ~ /^(✓|·)$/ {
        symbols++
        if ($3 != "✓") short[++faults] = $1 ": no def/sem rule is written for it"
        if ($5 != "✓") short[++faults] = $1 ": no Rust code claims its rules"
        if ($6 != "✓") short[++faults] = $1 ": no test claims its rules"
        if ($4 != "·") returned[++restored] = $1 ": carries a source annotation"
    }
    END {
        if (symbols != expected) {
            printf "check-port-ledger: %d symbols in the manifest, expected %d\n", symbols, expected > "/dev/stderr"
            printf "The port manifest is frozen because its source was retired. If it moved\n" > "/dev/stderr"
            printf "deliberately, move the expected count in this script to match.\n" > "/dev/stderr"
            exit 1
        }
        for (i = 1; i <= faults; i++)
            printf "check-port-ledger: %s\n", short[i] > "/dev/stderr"
        for (i = 1; i <= restored; i++)
            printf "check-port-ledger: %s\n", returned[i] > "/dev/stderr"
        if (restored > 0) {
            printf "A source annotation means the C corpus is back in this checkout. Put\n" > "/dev/stderr"
            printf "`nplan port check --wave=4` back in the release gate and delete this script.\n" > "/dev/stderr"
            exit 1
        }
        if (faults > 0) exit 1
        printf "port ledger: %d symbols, every rule written, claimed and tested; no source side, as retired\n", symbols
    }
'
