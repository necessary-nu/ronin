# The fold happens before the reference is read, so this `$` takes as its name
# whatever the fold left beside it -- the space the newline became, naming the
# unset variable ` `. Under `.POSIX:` the second fold's space is not swallowed
# by the first, so it survives the reference as value text.
.POSIX:
A = x$\
  \
  y
all: ; @printf '%s' '$(A)' | od -An -c > out
