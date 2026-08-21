# A Makefile made THROUGH a recursive prerequisite rather than by a recursive
# recipe of its own: `helper` carries the composed $(MAKE), and `gen.mk` is made
# from it. GNU Make brings helper up to date, runs the child, writes gen.mk,
# starts the read over once, and builds all from the new text.
#
# The recipe cut into segments has to stop being called begun once it is over.
# One that keeps saying it leaves helper dirty for the whole invocation, so
# gen.mk is remade on every pass, moves its own stamp, and the read never
# settles.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk: helper
	@printf 'GENERATED := from-generated\n' > $@

helper:
	@printf 'h\n' > helper
	@$(MAKE) -C sub marker
