# GNU Make recomputes MAKEFLAGS without -n, -t and -q for the makefile update
# and computes it again with them for the goals (define_makeflags, main.c: the
# three carry no_makefile in the switch table and nothing else does). So the
# $(MAKE) this remaking recipe holds starts a child that is not pretending
# either, and the childs own target is made for real.
#
# The recipe cannot compose: its $(MAKE) sits behind a `;` and is one command
# among several on the line, so it runs as a real nested process and learns
# what it was invoked under from MAKEFLAGS and from nowhere else.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk:
	@printf 'GENERATED := from-generated\n' > $@; $(MAKE) --no-print-directory -C sub marker
