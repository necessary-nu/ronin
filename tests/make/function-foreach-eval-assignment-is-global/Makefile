# The loop binding is scoped; what an assignment inside the loop defines is not.
$(foreach v,a b,$(eval seen_$(v) = yes))
all: ; @printf '%s\n' 'seen_a=[$(seen_a)] seen_b=[$(seen_b)] v=[$(v)] origin=$(origin v)' > out
