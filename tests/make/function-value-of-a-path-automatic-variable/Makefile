# The `D` and `F` forms are not computed per file the way the base forms are.
# GNU Make defines them once at startup, in `define_automatic_variables`, as
# recursive variables whose text is a `dir` or `notdir` expression over the base
# form. `value` reads back the text a variable was defined from, so for these it
# yields that expression rather than any path: `$(value @D)` is
# `$(patsubst %/,%,$(dir $@))` no matter which target is being built.
.PHONY: all
all: sub/a.o
	@printf '%s\n' 'ltD [$(value <D)]' 'ltF [$(value <F)]' \
		'hatD [$(value ^D)]' 'hatF [$(value ^F)]' 'qF [$(value ?F)]' >> out

sub/a.o:
	@mkdir -p $(@D)
	@printf '%s\n' 'atD [$(value @D)]' 'atF [$(value @F)]' \
		'expand atD [$(@D)]' 'expand atF [$(@F)]' > out
	@: > $@
