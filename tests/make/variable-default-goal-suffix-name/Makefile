# A target whose name is a declared suffix, or two of them joined, cannot
# become the default goal: Make passes over it and considers the next target.
.SUFFIXES: foo bar

foo: ; @printf '%s\n' 'foo ran' > foo.out
foobar: ; @printf '%s\n' 'foobar ran' > foobar.out
zed: ; @printf '%s\n' 'zed ran' > out
