# `export` in front of `define` is a modifier of that definition, so the
# multi-line value reaches the environment like any other.
export define D
l1
l2
endef
all: ; @printf 'D=[%s]\n' "$$D" > out
