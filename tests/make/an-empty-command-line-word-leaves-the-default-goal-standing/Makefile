# GNU Make's `handle_non_switch_argument` guards the goal branch with
# `arg[0] != '\0'`, so a zero-length argument word is neither a goal nor an
# assignment: the default goal stands and `MAKECMDGOALS` never hears about it.
# A wrapper expanding `make "$TARGET"` with `TARGET` unset hands Make exactly
# this word.
all: ; @printf 'built=[all] goals=[%s]\n' '$(MAKECMDGOALS)' > out
second: ; @printf 'built=[second]\n' >> out
