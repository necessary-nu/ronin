# Written down is a far weaker claim than declared: a name that carries nothing
# but target-specific variables has still reached the makefile's database, and
# the search will answer with it.
VPATH = d
d/fail.te: LOCAL = x
all: fail.te
	@printf 'all from %s\n' '$<' > seen
d/%.te: ; @printf 'pat %s\n' '$(LOCAL)' > $@
