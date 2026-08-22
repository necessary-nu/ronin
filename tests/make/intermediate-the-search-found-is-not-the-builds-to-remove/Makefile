# `f_mtime` searches VPATH for a name it cannot find where it was written, and
# the date the search brings back turns the bit off exactly as a local one does.
# Without GPATH nothing is renamed, so the build makes out.o where it was
# written and the copy in build/ is left where it lies.
VPATH = build

.INTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
