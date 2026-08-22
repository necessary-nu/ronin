# `ISBLANK` is a space or a tab, so a tab in front of the backslash is kept by
# the same rule the spaces are.
.POSIX:
A = a		\
  b
all: ; @printf '%s' '$(A)' | od -An -c > out
