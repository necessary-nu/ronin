# Two static pattern rules can name the same target with different patterns.
# Each prerequisite is built with the stem of the rule that introduced it, and
# the stem the recipe sees is the first rule's.
.PHONY: all all.one all.foo.two

all: all.foo.bar

all.foo.bar: %.foo.bar: %.one
all.foo.bar: %.bar: %.two

all.foo.bar: ; @printf '%s|%s\n' '$*' '$^' > out
