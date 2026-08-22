# The control, and the one that says the refusal is the update's rather than the
# switch's: `other` is a double-colon target and `-W` names it, but nothing the
# goals reach asks for it, so GNU Make never enters the fresh entry's update and
# the build runs as if the switch were not there.
all: app
app: dep
	touch app
other:: a
	touch other
other:: b
	touch other
