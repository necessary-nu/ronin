# The archive pass holds no directory aside: GNU Make sets `lastslash = 0` for
# it outright (reference/gnumake/src/implicit.c), so `(%)` matching
# `(d/foo.o)` takes the whole of `d/foo.o` for its stem and hands it back to
# the prerequisite unchanged. An archive in a subdirectory is the ordinary
# case and stays whole too.
AR = ./fake-ar
all: libx.a(d/foo.o) sub/liby.a(bar.o)
