# `check_specials` reads a rule's EXPANDED target names, so `$(P)` naming
# `.POSIX` turns the flag on as the line is read. kati reads the whole file
# before it evaluates `$(P):`, so the flag comes on for a fold already read;
# the reading is settled where the definition stands rather than at the fold.
P = .POSIX
$(P):
A = a   \
  b
all: ; @printf '[%s]\n' '$(A)' > out
