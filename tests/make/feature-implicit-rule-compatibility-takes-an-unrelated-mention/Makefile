# The name need not be written down anywhere near the target being made. A
# prerequisite of a rule that has nothing to do with this build is enough for
# the compatibility pass, and `hello` is then made in its own right.
all: hello.tsk

%.tsk: % ; @cat $< > $@
%: %.f ; @cat $< > $@
unrelated: hello
