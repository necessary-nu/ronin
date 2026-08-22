# `.INTERMEDIATE` is a bit on the file object too, and it travels the same way
# `.IGNORE` does: the object moves to the found path whole, so the file swept up
# afterwards is build/out.o.
VPATH = build
GPATH = build

.INTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
