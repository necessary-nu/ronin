# A bare `.SECONDARY:` makes every name intermediate, and `.NOTINTERMEDIATE`
# takes one back out — `remake.c` asks `!file->notintermediate` before it grants
# the blanket. The bit was declared for out.o and lands on the object standing
# at build/out.o, so this stale target is remade rather than forgiven.
VPATH = build
GPATH = build

.SECONDARY:
.NOTINTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
