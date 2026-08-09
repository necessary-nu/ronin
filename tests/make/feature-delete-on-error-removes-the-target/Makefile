# .DELETE_ON_ERROR says a target whose recipe failed is half-made and must not
# be left behind, including one made by a pattern rule.
.DELETE_ON_ERROR:

all: foo.bar

%.bar: ; @printf '%s\n' partial > $@; exit 1
