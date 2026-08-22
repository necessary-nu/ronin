# `precious` IS in `rehash_file`'s `MERGE` run, so unlike `.IGNORE` it reaches
# the found path whether or not something already stood there. build/out.o is
# named by a second rule and the protection still arrives.
VPATH = build
GPATH = build

.DELETE_ON_ERROR:
.PRECIOUS: out.o

all: out.o ; @printf 'all ran\n' > seen

other: build/out.o ; @printf 'other ran\n' > other-ran

out.o: out.c ; @printf 'made\n' > $@; exit 1
