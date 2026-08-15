# Being written down anywhere is enough for a name the makefile does not
# declare as a target: `d/fail.te` is only ever a prerequisite of something
# else, and the search still takes it — after which the pattern rule that
# matches the found path is what makes it.
VPATH = d
all: fail.te
	@printf 'all from %s\n' '$<' > seen
other: d/fail.te
d/%.te: ; @printf 'pat\n' > $@
