# Where the assignment lands does not depend on how it was spelled. What it
# reads while computing its value does: all three see the loop word.
p = outer
$(foreach p,a,$(eval p := simple-$(p)))
q = outer
$(foreach q,a,$(eval q ?= cond))
r = outer
$(foreach r,a,$(eval override r = ovr))
all: ; @printf '%s\n' 'p=[$(p)] $(flavor p) $(origin p)' 'q=[$(q)] $(flavor q) $(origin q)' 'r=[$(r)] $(origin r)' > out
