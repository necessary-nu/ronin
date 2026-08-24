# A target-specific `+=' is recursive whatever the base's flavour, and its tail
# is expanded in the scope that READS it, not where it was written. `a.o''s own
# recipe reads W as `aoW'; `dep'', which inherits the binding down the
# prerequisite link, reads its own `depW'. The private pattern binding shows the
# base is read per reader too: `a.o'' sees the `%.o'' private `p', `dep'' does
# not.
V = g
W = outerW
%.o: private V += p
a.o: V += <$(W)>
a.o: W = aoW
dep: W = depW
all: a.o
a.o: dep ; @printf '%s' "$(V)" > aout
dep: ; @printf '%s' "$(V)" > dout
