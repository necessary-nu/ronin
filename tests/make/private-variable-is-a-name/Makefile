private = g
all: private = a
all: dep export override private foo bar
dep: ; @printf '[%s]\n' '$(private)' > dep.out
foo bar export override private: ; @printf '%s\n' '$@' > $@.out
all: ; @printf '[%s]\n' '$(private)' > all.out
