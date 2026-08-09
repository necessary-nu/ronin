all: $(shell printf 'p1 p2\n') ; @printf '[%s]\n' '$^' > out
p1 p2: ; @printf '%s\n' '$@' > $@
