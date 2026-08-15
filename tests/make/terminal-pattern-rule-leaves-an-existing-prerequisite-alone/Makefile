# `hello.x' is there but out of date against `hello.y'. A terminal rule took
# it as it found it, so nothing remakes it and `hello.z' is made from the old
# contents.
all: hello.z
%.z:: %.x ; cat $< > $@
%.x: %.y ; cat $< > $@
