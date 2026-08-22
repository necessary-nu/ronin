# `secondary` is in the `MERGE` run, so the found path's own `.INTERMEDIATE` and
# the written name's `.SECONDARY` land on one object. `remove_intermediates`
# (file.c) asks for `intermediate` and against `secondary` separately, so the
# file is intermediate and is still not swept up.
VPATH = build
GPATH = build

.INTERMEDIATE: build/out.o
.SECONDARY: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
