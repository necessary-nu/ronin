# Only the first `%` of a deferred prerequisite stands for the stem. GNU Make
# walks the word replacing that one with `$*` and copies the rest through, so a
# later percent reaches the prerequisite name as a literal percent.
.SECONDEXPANSION:
all: bye.x

%.x: $$(wordlist 1, 99, %1%2%) ; @printf '%s\n' '$^' > log; touch $*.x

.PHONY: bye1%2%
