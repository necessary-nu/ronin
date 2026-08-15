# The library search asks the filesystem for the working directory and for the
# compiled-in system directories, but its VPATH tier is the ordinary search and
# inherits the ordinary search's answers: a `.LIBPATTERNS` candidate that the
# makefile can make in a searched directory resolves and is built.
.LIBPATTERNS = made_%.a
vpath %.a d/
all: -lfoo ; @printf 'from %s\n' '$<' > seen
d/made_foo.a: ; @printf 'lib\n' > $@
