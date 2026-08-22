# What a dependent sees through an intermediate is the newest thing under it,
# which is why the walk carries the DEPENDENT's date down rather than the
# intermediate's: out.c is newer than app though build/out.o is older than both.
VPATH = build
GPATH = build

.SECONDARY: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
