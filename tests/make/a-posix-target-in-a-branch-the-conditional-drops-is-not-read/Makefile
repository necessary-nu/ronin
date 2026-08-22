# GNU Make chooses the branch as it reads, so a line in the branch that loses is
# never read and `check_specials` never sees it. The read here buffers both
# branches for the evaluation to choose between, so a `.POSIX` written in either
# is one this read must leave alone.
ifeq (1,0)
.POSIX:
endif

A = a   \
  b

all: ; @printf '[%s]\n' '$(A)' > out
