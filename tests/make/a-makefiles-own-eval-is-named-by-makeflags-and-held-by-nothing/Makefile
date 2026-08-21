# The other half of that stickiness, and the shape that shows the reference is
# a reference rather than a spelling of the fragments.
#
# A `--eval` a makefile writes into MAKEFLAGS reaches GNU Make's `eval_strings`
# — so the reference is written — and does not reach `-*-eval-flags-*-`, which
# `main` defined once and never writes again. This invocation carried no
# fragment, so the name is undefined, the reference resolves to nothing, and
# `$(MAKEFLAGS)` ends in the space that was to precede it. The fragment itself
# is not evaluated either: `Y` stays empty, because `eval_buffer` ran before
# the makefile was read.
MAKEFLAGS := --eval=Y=2

$(file > read,mf=[$(MAKEFLAGS)] val=[$(value MAKEFLAGS)])
$(file >> read,ef=[$(-*-eval-flags-*-)] origin=[$(origin -*-eval-flags-*-)] Y=[$(Y)])

all:
	@printf 'ran\n' > ran
