# An unexpanded $(MAKE) reference classifies the line recursive for GNU Make,
# which then runs it under -n whatever the line actually does. The same value
# reached through another variable is not the same reference, and is not
# classified.
MAKE_ALIAS = $(MAKE)
all:
	test -n "$(MAKE)" && echo named > named
	test -n "$(MAKE_ALIAS)" && echo aliased > aliased
