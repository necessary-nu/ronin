# The same in the other direction: a file a wildcard found while reading is
# gone from the wildcard in a recipe expanded after another recipe removed it.
$(shell printf 'x\n' > early.probe)
reading := $(wildcard *.probe)

all: second
first: ; @rm -f early.probe
second: first ; @printf '[%s] [%s]\n' '$(reading)' '$(wildcard *.probe)' > out
.PHONY: all first second
