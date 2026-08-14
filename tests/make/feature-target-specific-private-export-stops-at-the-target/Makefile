# `private` withholds the binding from every scope that reaches this one
# through a parent, and a prerequisite's recipe is one of those scopes.
all: base ; @printf 'all=%s\n' "$$P" > all-out
all: private export P = p
base: ; @printf 'base=%s\n' "$$P" > base-out
