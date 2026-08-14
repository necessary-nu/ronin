# `unexport` in front of a target-specific definition is `export`'s mirror: the
# definition happens, and the name is withheld from that target's environment
# even though the global binding is exported.
export W = global
all: base ; @printf 'env=[%s] make=[%s]\n' "$$W" "$(W)" > all-out
all: unexport W = local
base: ; @printf 'env=[%s]\n' "$$W" > base-out
