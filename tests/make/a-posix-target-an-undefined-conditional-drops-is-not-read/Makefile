# The same for `ifdef`, which is the plausible spelling of it: a makefile that
# offers POSIX behind a variable must not get it when the variable is unset.
ifdef POSIXLY_CORRECT
.POSIX:
endif

A = a   \
  b

all: ; @printf '[%s]\n' '$(A)' > out
