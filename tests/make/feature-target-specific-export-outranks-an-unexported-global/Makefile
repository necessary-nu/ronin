# The other direction: a global the makefile withdrew, exported again for one
# target alone.
V = global
unexport V
all: base ; @printf 'all=[%s]\n' "$$V" > all-out
all: export V = local
base: ; @printf 'base=[%s]\n' "$$V" > base-out
