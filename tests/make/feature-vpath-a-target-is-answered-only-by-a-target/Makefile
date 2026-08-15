# The bar moves with what is being searched for. `fail.te` is a target here, so
# only another target answers for it — a `d/fail.te` that is merely mentioned
# does not, and the rule written here is what runs.
VPATH = d
all: fail.te
	@printf 'all from %s\n' '$<' > seen
fail.te: ; @printf 'here\n' > $@
other: d/fail.te
