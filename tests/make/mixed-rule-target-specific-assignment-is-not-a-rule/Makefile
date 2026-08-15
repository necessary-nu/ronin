# A target list is only obliged to be all one kind once it has to become a
# rule. A target-specific assignment is not a rule, so neither order of a mixed
# list is diagnosed at all, and both bindings land.
a %.o: V = one
%.d b: W = two
all: a b ; @printf top > out
a: ; @printf 'a %s\n' '$(V)' > $@
b: ; @printf 'b %s\n' '$(W)' > $@
