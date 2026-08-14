# A makefile-set `-r` takes the suffix list away with the rules, but only
# while the list is still Make's own. Naming `.SUFFIXES` makes the list this
# makefile's, so `.c` survives — and a declared suffix withdraws the
# match-anything rule from a name that ends in it, whether or not any rule
# was left to make that name.
MAKEFLAGS += -r
.SUFFIXES: .c

%: ; @printf 'anymatch\n' > $@

all: foo.c
