# `+=` folds each append where it is written: the first is read before the
# `.POSIX:` the eval sets and folds plain, the second after it and the POSIX
# way. One flag on the variable could not tell the two apart, so the reading is
# settled per definition -- where GNU Make folds it.
A = a   \
  b
$(eval .POSIX:)
A += c   \
  d
all: ; @printf '[%s]\n' '$(A)' > out
