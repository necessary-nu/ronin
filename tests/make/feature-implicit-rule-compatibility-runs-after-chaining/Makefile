# Chaining is tried before the compatibility pass too: `%.tsk: %.z` can invent
# `hello.z` from `hello.y`, and that beats `%.tsk: %` taking `hello` on trust
# despite being written second.
all: hello.tsk

%.tsk: % ; @printf '%s\n' compat > $@
%.tsk: %.z ; @printf '%s\n' chained > $@
%.z: %.y ; @cat $< > $@
unrelated: hello
