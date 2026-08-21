# The compiler's preferred recursion shape, on the makefile that has to be
# remade before the read means anything: the `$(MAKE)` line's own command is
# the invocation, so it is composed into the graph rather than left in the
# recipe. The wrapper edge is therefore waiting for a child while the makefile
# update wants to run it, and running an unfinished wrapper runs the freshness
# probe. GNU Make remakes gen.mk, runs the child, starts the read over on the
# new text and builds all from it.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk:
	@printf 'GENERATED := from-generated\n' > $@
	@$(MAKE) -C sub marker
