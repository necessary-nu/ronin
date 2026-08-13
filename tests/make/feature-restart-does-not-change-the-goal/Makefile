# Remaking a Makefile is not asking for it: the goal named on the command line
# is still the goal after the read starts again, and the generated include has
# not become one.
all: ; @printf 'all restarts=%s\n' '$(MAKE_RESTARTS)' > out-all
other: ; @printf 'other restarts=%s\n' '$(MAKE_RESTARTS)' > out-other

-include gen.mk

gen.mk: ; @printf '# made\n' > $@
