# The first-percent rule is per whitespace-separated segment rather than per
# word: after skipping to the next blank GNU Make looks for a percent again, so
# both of these are placeholders even though they share one variable reference.
.SECONDEXPANSION:
all: bye.x

%.x: $$(wordlist 1, 99, %a %b) ; @printf '%s\n' '$^' > log; touch $*.x

.PHONY: byea byeb
