# The other side of the branch, and the one that is still divergent: GNU Make
# has chosen the branch by the time it reads the line below, and this read has
# not.
ifeq (1,1)
.POSIX:
endif

A = a   \
  b

all: ; @printf '[%s]\n' '$(A)' > out
