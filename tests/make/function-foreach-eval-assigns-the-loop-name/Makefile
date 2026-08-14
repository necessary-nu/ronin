# Naming the loop variable itself: the binding goes on shadowing every read for
# the rest of the loop, and the assignment is waiting once the loop unwinds.
v = outer
inner := $(foreach v,a b,[$(v)]$(eval v = hijack)[$(v)][$(origin v)])
all: ; @printf '%s\n' '$(inner)' 'after [$(v)] $(flavor v) $(origin v)' > out
