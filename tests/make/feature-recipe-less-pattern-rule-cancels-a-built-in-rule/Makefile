# The documented use of the form. GNU Make installs its built-in rules after
# the makefiles, and one whose name a written rule already holds is thrown
# away, so nothing is left that can compile foo.c. Ronin installs no built-in
# rules at all and arrives at the same place by the shorter road.
%.o: %.c

all: foo.o
