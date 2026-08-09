# The two target patterns are distinct candidates for one rule. Their stems
# produce different second-expanded prerequisites, so rejecting the shorter
# stem must not cache its prerequisite for the candidate that can succeed.
.SECONDEXPANSION:
all: ab.x

%.x a%.x: $$(addsuffix .ok,$$*) ; @printf 'stem=%s\n' '$*' > log; touch ab.x
