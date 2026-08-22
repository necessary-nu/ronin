# `collapse_continuations` (misc.c) discards the blanks it has already written
# before it writes the fold's space, and under `.POSIX:` it discards nothing:
# each backslash-newline is a space ADDED to what was there rather than a space
# standing in for it. The blanks after the newline go either way.
.POSIX:
A = a   \
  b
all: ; @printf '%s' '$(A)' | od -An -c > out
