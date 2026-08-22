# `find -exec ... \;` ends on a `;` the backslash makes an argument. A line
# that lost it would end on the backslash instead, where it escapes whatever
# separates this line from the next.
all:
	@find . -name Makefile -exec cp {} m1 \;
	@touch m2
