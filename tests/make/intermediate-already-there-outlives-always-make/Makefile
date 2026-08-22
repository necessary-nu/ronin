# -B remakes the file and does not make it the build's: what decides that is the
# date the first stat found, and -B does not change what was lying there.
.INTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
