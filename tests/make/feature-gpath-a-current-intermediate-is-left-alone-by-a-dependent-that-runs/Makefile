# The second loop updates an intermediate rather than forcing it: build/out.o is
# newer than out.c, so the dependent runs and the intermediate does not, and app
# is written from the bytes that were already there.
VPATH = build
GPATH = build

.SECONDARY: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
