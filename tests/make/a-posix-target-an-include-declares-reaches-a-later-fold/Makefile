# The included file turns the flag on, and the fold below the `include` in the
# includer was read whole before the include was evaluated.
include inc.mk
A = a   \
  b
all: ; @printf '[%s]\n' '$(A)' > out
