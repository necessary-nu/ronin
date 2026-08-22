# `intermediate` is the field `rehash_file`'s `MERGE` run deliberately leaves
# out, with a comment saying why: the object standing at the found path may have
# been there before make was. So naming build/out.o keeps `.INTERMEDIATE: out.o`
# from reaching it, and the file survives the build.
VPATH = build
GPATH = build

.INTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

other: build/out.o ; @printf 'other ran\n' > other-ran

out.o: out.c ; @printf 'made\n' > $@
