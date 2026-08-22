# `f_mtime` (remake.c) turns the declared intermediate bit off for a file it has
# just found a date for and has never built: the file was there before make
# started, so make did not create it and must not delete it. The recipe still
# runs — out.o is older than out.c — and the file it rewrote stays behind.
.INTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
