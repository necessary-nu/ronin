# `+=` appends to what the rule's own scope already holds rather than to the
# makefile-level value, for a named target and for a pattern alike.
N = global
M = global

t.q: N = own
t.q: N += more

%.q: M = own
%.q: M += more

all: t.q
t.q: ; @printf 'N=%s M=%s\n' '$(N)' '$(M)' > $@
