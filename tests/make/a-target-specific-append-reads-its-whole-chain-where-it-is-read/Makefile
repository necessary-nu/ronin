# The whole `+=' chain -- base and tail alike -- is re-expanded in the scope
# that reads it, because a target-specific `+=' is a recursive value GNU Make's
# variable_append walks per reader. `a.o'' reads its own X and W; `dep'', which
# inherits the binding, reads its own. A string settled where the `+=' was
# written could carry neither.
X = outerX
V = base<$(X)>
W = outerW
a.o: V += tail<$(W)>
a.o: X = aoX
a.o: W = aoW
dep: X = depX
dep: W = depW
all: a.o
a.o: dep ; @printf '%s' "$(V)" > aout
dep: ; @printf '%s' "$(V)" > dout
