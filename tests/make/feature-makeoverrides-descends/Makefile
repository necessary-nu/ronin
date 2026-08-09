# MAKEOVERRIDES holds what was assigned on the command line, and a sub-make
# reads the same list.
all:
	@printf '%s\n' '$(MAKEOVERRIDES)|$(origin MAKEOVERRIDES)|$(flavor MAKEOVERRIDES)|$(origin MAKEFLAGS)|$(flavor MAKEFLAGS)' > top
	$(MAKE) --no-print-directory -f sub.mk subgoal
