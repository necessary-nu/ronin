# A pattern variable applies only where the pattern leaves a stem to match, so
# `foo%bar` reaches `fooXbar` and not `foobar`, which it is longer than.
foo%bar: N = from-pattern

all: foobar fooXbar
foobar fooXbar: ; @printf '[%s]\n' '$(N)' > $@
