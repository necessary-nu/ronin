# The same reading in the other direction: the second target's percent is
# escaped, so it does not agree with the first target's, and a pattern rule
# with a plain target among its targets stops the read.
all: foo.o ; @printf top > out
%.o \%.x: ; @printf 'ran %s\n' '$@' > $@
