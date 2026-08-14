# A `private` binding is invisible from every scope but its own — not merely
# unexported. A target-specific assignment beside one is left to answer for
# itself, and answers no, where the same assignment beside a public exported
# global would have taken its yes.
private export HIDDEN := global
export SHOWN := global
all: one two ; @printf 'all H=[%s] S=[%s]\n' "$$HIDDEN" "$$SHOWN" > all-out
one: HIDDEN := local
one: SHOWN := local
one: ; @printf 'one H=[%s] S=[%s]\n' "$$HIDDEN" "$$SHOWN" > one-out
two: ; @printf 'two H=[%s] S=[%s]\n' "$$HIDDEN" "$$SHOWN" > two-out
