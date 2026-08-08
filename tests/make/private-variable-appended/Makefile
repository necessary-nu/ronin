DEFS = FOO
all: bar1
bar1: private DEFS += 1
bar3: private DEFS += 3
bar1: bar2
bar2: bar3
bar1 bar2 bar3: ; @printf '%s\n' '$(DEFS)' > $@.out
all: ; @true
