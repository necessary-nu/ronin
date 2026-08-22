# `.PHONY` alone makes the found name a target, so the search takes it and the
# merge happens with an object that has no recipe: the one written here runs,
# under the found name.
vpath %.te d/

all: fail.te
	@printf 'all from [%s]\n' '$<' > seen

.PHONY: d/fail.te
fail.te: ; @printf 'here [%s]\n' '$@' > seen2
