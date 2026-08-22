# The control for the one beside it: the same makefile with nothing at out.o, so
# the build is what put the file there and the sweep is entitled to it.
.INTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
