# -include says the file may be absent, and that carries through to the rule
# that would have made it: the recipe runs, it fails, and the build goes on
# without the include rather than stopping.
all: ; @printf 'all ran\n' > out

-include gen.mk

gen.mk: ; @printf 'gen ran\n' > log; exit 1
