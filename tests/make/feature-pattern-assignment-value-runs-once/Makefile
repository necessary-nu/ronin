# A value with a side effect runs while the makefile is read, once for each
# pattern the assignment names, and not once per target that matches.
%.a %.b: SIDE := $(shell printf 'ran\n' >> side.log; printf 'value')

all: x.a
x.a: ; @printf '%s\n' '$(SIDE)' > $@
