# The edge is gone from the list rather than merely stepped over, so a second
# target that reaches the same loop finds it already broken and the recipe runs
# once.
all: p q
p: a ; @printf 'p\n' >> order
q: a ; @printf 'q\n' >> order
a: b ; @printf 'a hat=[$^]\n' >> order
b: a ; @printf 'b hat=[$^]\n' >> order
