# Two names the Makefile declared separately, which the search finds to be one
# file. The object standing at the found path is the one kept, so its recipe is
# what runs and its name is what dependents read; the recipe written here is
# dropped and said to be dropped.
vpath %.te d/

all: fail.te
	@printf 'all from [%s]\n' '$<' > seen

fail.te: ; @printf 'here [%s]\n' '$@' > seen2; echo here > $@
d/fail.te: ; @printf 'there [%s]\n' '$@' > seen3; echo there > $@
