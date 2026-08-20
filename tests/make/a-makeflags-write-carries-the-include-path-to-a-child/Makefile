# `define_makeflags` writes every entry of the `-I` list into MAKEFLAGS, one
# `-I<dir>` word each, and that is the only way a child learns the search path:
# `$(MAKE)` is one word and carries nothing. The child's fragment is reachable
# only through the directory this makefile's own write named.
MAKEFLAGS += -I inc2

all: ; @$(MAKE) --no-print-directory -f child.mk
