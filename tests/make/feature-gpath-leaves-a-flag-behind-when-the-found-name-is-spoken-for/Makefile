# The other ending of `rehash_file`: an object already standing at the found
# path is RETAINED and the searched name's is merged into it, and the closing
# run of `MERGE(field)` does not name `command_flags`. Naming build/out.o
# anywhere is enough to put an object there, so `.IGNORE: out.o` does not reach
# it and the failure counts.
VPATH = build
GPATH = build

.IGNORE: out.o

all: out.o ; @printf 'all ran\n' > seen

other: build/out.o ; @printf 'other ran\n' > other-ran

out.o: out.c ; @printf 'made\n' > $@; exit 1
