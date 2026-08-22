# The control for the run: without `.POSIX:` the whole of it, blanks and all,
# comes out as a single space.
A = a  \
  \
  b
all: ; @printf '%s' '$(A)' | od -An -c > out
