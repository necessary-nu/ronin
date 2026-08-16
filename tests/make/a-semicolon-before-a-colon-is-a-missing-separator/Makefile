# The semicolon takes no part in the assignment question and all of it in the
# rule one. There is no assignment operator on this line, so GNU Make reads it
# as a rule — and a rule is cut at the first unquoted semicolon before the colon
# is looked for, which leaves `a` with no separator at all.
a;b: c
c: ; @printf 'made c\n' > out
all: ; @printf 'made all\n' >> out
