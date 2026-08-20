# GNU Make searches the `-I` directories and then a BUILT-IN default path —
# `/usr/gnu/include`, `/usr/local/include`, `/usr/include`, whichever of them
# are on disk (`default_include_directories`, read.c:103). So a fragment nobody
# named a directory for is still found, and this case says so by including one
# that is certainly there and certainly not a makefile: a build that finds
# `stdio.h` stops on its first line, and a build that does not find it carries
# on and writes `out`.
#
# The `-` is what makes the two outcomes distinguishable. Without it, a fragment
# that is not found anywhere is a target Make must remake, and both tools refuse
# alike; with it, not finding the file is the ordinary, silent, successful case.
-include stdio.h

all: ; @echo built > out
