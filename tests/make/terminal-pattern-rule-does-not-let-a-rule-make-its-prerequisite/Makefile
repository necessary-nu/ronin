# `%.z::' is terminal, so `hello.x' has to be there already. It is written
# down on `unrelated:', which is enough for the rule to be chosen and not
# enough to make it: the `%.x:' rule below may not be consulted for a name a
# terminal rule was handed.
all: hello.z
%.z:: %.x ; touch $@
%.x: ;
unrelated: hello.x
