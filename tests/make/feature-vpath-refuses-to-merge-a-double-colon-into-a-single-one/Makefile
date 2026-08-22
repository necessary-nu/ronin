# The same refusal the other way round.
vpath %.te d/

all: fail.te
	@printf 'all from [%s]\n' '$<' > seen

fail.te:: ; @printf 'here [%s]\n' '$@' > seen2
d/fail.te: ; @printf 'there [%s]\n' '$@' > seen3
