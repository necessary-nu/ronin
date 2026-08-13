# -n asks what the goals would do. Remaking a Makefile is not one of the things
# it would do: without the file there is no telling what the goals are, so make
# makes it for real, starts the read again, and only then prints.
all: ; @printf 'restarts=%s value=%s\n' '$(MAKE_RESTARTS)' '$(V)' > out

-include gen.mk

gen.mk: ; @printf 'V := built\n' > $@
