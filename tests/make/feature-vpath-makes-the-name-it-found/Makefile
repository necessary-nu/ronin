# The name the search found is the name the build makes and the name its
# dependents read, so the rule declared for the found path is the one that runs.
vpath %.te d/
all: fail.te
	@printf 'all from %s\n' '$<' > seen
d/fail.te: ; @printf 'made\n' > $@
