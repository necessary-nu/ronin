# `check_specials` (read.c) reads the target names after they are expanded, so
# a `.POSIX` only a variable spells still sets the flag. What is read after that
# folds the POSIX way, and what an `include` brings in below it is read after it.
P = .POSIX
$(P):
include inc.mk
all: ; @printf '%s' '$(A)' | od -An -c > out
