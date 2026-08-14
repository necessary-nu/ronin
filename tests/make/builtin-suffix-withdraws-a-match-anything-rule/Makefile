# `.c` is a built-in suffix, which is by itself a rule that matches `foo.c`
# and makes nothing. Matching it is what strikes the match-anything rule out
# of the search, so `foo.c` has no rule at all.
%: ; @printf 'anymatch\n' > $@

all: foo.c
