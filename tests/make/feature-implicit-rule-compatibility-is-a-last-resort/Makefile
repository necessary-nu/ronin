# The compatibility pass runs after every rule has been tried on its merits, so
# a rule whose prerequisite is really on disk wins over one that would have to
# take `hello` on trust, even though it is written second.
all: hello.tsk

%.tsk: % ; @printf '%s\n' compat > $@
%.tsk: %.f ; @printf '%s\n' existing > $@
unrelated: hello
