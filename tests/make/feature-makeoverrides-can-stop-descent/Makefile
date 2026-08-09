# Clearing MAKEOVERRIDES removes command-line precedence from the recursive
# MAKEFLAGS value before the child Makefile is compiled. The ordinary exported
# value still arrives with environment origin.
MAKEOVERRIDES =
all:
	$(MAKE) --no-print-directory -f child.mk child
.PHONY: all
