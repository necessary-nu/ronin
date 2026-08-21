# GNU Make writes the fragments' reference into MAKEFLAGS whenever it has ever
# seen a `--eval` at all, and `eval_strings` — the list it asks — is only ever
# appended to. A makefile that writes MAKEFLAGS away therefore cannot take the
# invocation's fragments off it: `define_makeflags` puts the reference back,
# and the reference still names the variable `main` filled at startup.
#
# The write below replaces the whole value rather than appending to it, which
# is the case that tells a reference apart from inlined text. Inlined text is
# gone the moment the value it was inlined into is replaced.
MAKEFLAGS := -k

$(file > read,mf=[$(MAKEFLAGS)] val=[$(value MAKEFLAGS)])
$(file >> read,ef=[$(-*-eval-flags-*-)] origin=[$(origin -*-eval-flags-*-)])

all:
	@printf 'ran\n' > ran
