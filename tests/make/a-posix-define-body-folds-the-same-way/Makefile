# A `define` body is read line by line like anything else, so its continuations
# answer to `.POSIX:` too.
.POSIX:
define A
a   \
  b
endef
all: ; @printf '%s' '$(A)' | od -An -c > out
