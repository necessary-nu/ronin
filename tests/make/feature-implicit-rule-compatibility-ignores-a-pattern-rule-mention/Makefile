# A pattern rule's prerequisite is not a name the Makefile writes down: until
# the rule is applied there is no such file to know about. `%.zz: hello` is the
# only mention of `hello`, so the compatibility pass does not reach it and the
# build is refused.
all: hello.tsk

%.tsk: % ; @cat $< > $@
%: %.f ; @cat $< > $@
%.zz: hello ; @printf '%s\n' zz > $@
