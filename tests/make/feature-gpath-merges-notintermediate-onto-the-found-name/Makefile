# The same reading for `notintermediate`, which is in the `MERGE` run beside
# `secondary`. Writing both for ONE name is a read-time error, so a rename is
# the only way a file object comes to hold `.INTERMEDIATE` and
# `.NOTINTERMEDIATE` at once — and then the sweep leaves it alone.
VPATH = build
GPATH = build

.INTERMEDIATE: build/out.o
.NOTINTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
