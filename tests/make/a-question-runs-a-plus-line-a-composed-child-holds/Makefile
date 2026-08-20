# The recursion is composed into one graph rather than started as a child, so
# the question walks the child's own edges. A `+` line the child holds is
# marked there exactly as it would be here, and runs while the question is
# being asked -- which is what GNU Make's child, itself invoked under `-q`,
# does with the same line.
all:
	+$(MAKE) -C sub child
