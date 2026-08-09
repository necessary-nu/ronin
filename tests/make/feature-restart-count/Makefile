# MAKE_RESTARTS counts the times make has started itself again to pick up a
# rebuilt makefile: empty on the first pass, one afterwards.
all: ; @printf 'restarts=%s\n' '$(MAKE_RESTARTS)' > out

include gen.mk

gen.mk: ; @printf '# made\n' > $@
