# One object cannot be both a single-colon and a double-colon target, so the
# merge the search asked for is refused outright rather than resolved.
vpath %.te d/

all: fail.te
	@printf 'all from [%s]\n' '$<' > seen

fail.te: ; @printf 'here [%s]\n' '$@' > seen2
d/fail.te:: ; @printf 'there [%s]\n' '$@' > seen3
