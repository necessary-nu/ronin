# An order-only prerequisite is on the same list GNU Make walks, marked rather
# than held apart, so one that closes a loop is dropped like any other.
all: a
a: b ; @printf 'a bar=[$|]\n' > a.out
b: | a ; @printf 'b bar=[$|]\n' > b.out
