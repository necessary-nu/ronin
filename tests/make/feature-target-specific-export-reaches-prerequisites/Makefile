# A target-specific export is inherited the way a target-specific value is:
# down the whole chain of prerequisites, and not sideways to a target that
# reaches it no other way.
all: mid other ; @printf 'all=%s\n' "$$TS" > all-out
all: export TS = t
mid: leaf ; @printf 'mid=%s\n' "$$TS" > mid-out
leaf: ; @printf 'leaf=%s\n' "$$TS" > leaf-out
other: ; @printf 'other=%s\n' "$$TS" > other-out
