# The `.POSIX:` is one include deeper, and the fold in the outermost file was
# read before either include was evaluated.
include inc.mk
A = a   \
  b
all: ; @printf '[%s]\n' '$(A)' > out
