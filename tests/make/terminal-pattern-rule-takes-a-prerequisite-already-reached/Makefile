# The same rules as the case beside this one, with the goals the other way
# round: `other' reaches `hello.x' first and gives it the `%.x:' rule, so by
# the time the terminal rule is chosen the bar it sets changes nothing.
all: other hello.z
%.z:: %.x ; touch $@
%.x: ; touch $@
other: hello.x ; touch other
