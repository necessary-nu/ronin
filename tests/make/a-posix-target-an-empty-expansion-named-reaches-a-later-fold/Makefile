# The special target is spelled with empty expansions around it, so only the
# evaluation can read `.POSIX` out of the line. The fold below it was read first.
E =
$(E).POSIX$(E):
A = a   \
  b
all: ; @printf '[%s]\n' '$(A)' > out
