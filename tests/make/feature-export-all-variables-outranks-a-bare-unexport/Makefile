# `.EXPORT_ALL_VARIABLES:` is a target, and its targets are read after the
# whole makefile is, so it settles the same switch last however early it is
# written.
one = 1
.EXPORT_ALL_VARIABLES:
unexport
all: ; @printf 'one=%s\n' "$$one" > out
