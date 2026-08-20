# The halves are spelt against the directory the command runs in, exactly as
# the whole list is: a recursive child names its prerequisites the way its own
# Makefile did, so `$(?D)` is `d .` there and never `sub/d sub`.
all: ; @$(MAKE) -C sub
