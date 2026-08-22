# The variable set LISTS are merged rather than their contents, so a name only
# the searched target bound is still readable and a name both bound is the kept
# target's.
vpath %.te d/

all: fail.te
	@printf 'all from [%s]\n' '$<' > seen

fail.te: ONLY_HERE = fromhere
fail.te: BOTH = fromhere
d/fail.te: ONLY_THERE = fromthere
d/fail.te: BOTH = fromthere
d/fail.te: ; @printf 'here [%s] there [%s] both [%s]\n' '$(ONLY_HERE)' '$(ONLY_THERE)' '$(BOTH)' > seen2
