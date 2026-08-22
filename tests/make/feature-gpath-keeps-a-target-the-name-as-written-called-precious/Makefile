# `.DELETE_ON_ERROR` takes back what a failed recipe half-wrote, and `.PRECIOUS`
# stops it. The protection was declared for out.o and the file the recipe wrote
# is build/out.o, which is the same object under a name `GPATH` gave it.
VPATH = build
GPATH = build

.DELETE_ON_ERROR:
.PRECIOUS: out.o

all: out.o ; @printf 'all ran\n' > seen

out.o: out.c ; @printf 'made\n' > $@; exit 1
