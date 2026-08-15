# A pattern is not a name. `d/%.te` says how a name in that directory could be
# made and never writes one down, so the search passes the directory over and
# the implicit rule search runs on the name as written.
VPATH = d
all: fail.te
	@printf 'all from %s\n' '$<' > seen
%.te: ; @printf 'here\n' > $@
d/%.te: ; @printf 'there\n' > $@
