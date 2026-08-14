# `flavor` and `origin` have to tell the same story `value` does. Every
# automatic variable in scope has origin `automatic`; the flavor is what
# separates the two kinds. A base form was defined as a simple variable holding
# the computed name, and a `D`/`F` form as a recursive one holding the
# expression that computes it.
#
# `$|` is the exception with no `D` or `F` form at all, so `|D` is a name nobody
# defined and reads as undefined rather than as an automatic variable.
.PHONY: all ord
all: dep | ord
	@printf '%s\n' 'at $(flavor @) $(origin @)' 'atD $(flavor @D) $(origin @D)' \
		'atF $(flavor @F) $(origin @F)' 'lt $(flavor <) $(origin <)' \
		'hat $(flavor ^) $(origin ^)' 'star $(flavor *) $(origin *)' \
		'q $(flavor ?) $(origin ?)' 'bar $(flavor |) $(origin |)' \
		'barD $(flavor |D) $(origin |D)' > out

dep: ; @: > $@

ord: ; @:
