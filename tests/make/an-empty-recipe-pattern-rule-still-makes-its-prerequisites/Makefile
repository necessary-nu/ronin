# A pattern rule whose recipe is empty still has prerequisites, and they still
# have to be made: an empty recipe means the rule runs nothing, not that it
# wants nothing. GNU Make's `pattern_search` only passes over a pattern rule
# with prerequisites and *no* recipe at all — the cancellation a Makefile writes
# to unmake a built-in rule — and `%.q: %.o ;` is not that (src/implicit.c,
# `rule->deps != 0 && rule->cmds == 0`).
#
# `x.o` is the delicate half: nothing writes it down, so the search invents it
# as an intermediate, and an intermediate is excused from existing. What asks
# for it anyway is the target above it not being there — GNU Make's `must_make`
# is set for a file that does not exist, whatever its prerequisites say — and
# `x.q` is never written by anything, because the recipe that would write it is
# the empty one.
all: x.q
	@echo TOP >> log

%.q: %.o ;

%.o : %.c
	@echo Compile $< >> log; touch $@
