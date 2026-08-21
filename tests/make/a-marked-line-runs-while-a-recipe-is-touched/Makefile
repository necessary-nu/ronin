# The other side of `a-touch-over-a-composed-recipes-marked-line`: a recipe
# whose lines are the launches gets the `+` marker per line, which is what GNU
# Make does. Under `-t` the marked line runs and the rest are stood in for, and
# the target is touched rather than made.
goal:
	@echo one > one.out
	+@echo marked > marked.out
	@echo two > two.out
