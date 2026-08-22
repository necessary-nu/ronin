.POSIX:

# The same length as the case beside this one, and a shell named by the
# Makefile, which is what sends the launch down the composed command line
# instead of the direct one.
P1 := xxxxxxxxxx
P2 := $(P1)$(P1)$(P1)$(P1)$(P1)$(P1)$(P1)$(P1)$(P1)$(P1)
P3 := $(P2)$(P2)$(P2)$(P2)$(P2)$(P2)$(P2)$(P2)$(P2)$(P2)
P4 := $(P3)$(P3)$(P3)$(P3)$(P3)$(P3)$(P3)$(P3)$(P3)$(P3)
PAD := $(P4)$(P4)$(P4)$(P4)$(P4)$(P4)$(P4)$(P4)$(P4)$(P4)$(P4)

SHELL = /bin/bash

all: out

out:
	@: $(PAD) ; false ; touch m1
