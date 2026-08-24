# A `define` body's continuations answer to `.POSIX:` too, including one the
# evaluation reaches: the body was read whole before the eval ran.
$(eval .POSIX:)
define A
a   \
  b
endef
all: ; @printf '[%s]\n' '$(A)' > out
