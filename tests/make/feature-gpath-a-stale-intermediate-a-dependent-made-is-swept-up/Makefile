# `.INTERMEDIATE` rather than `.SECONDARY`, so the file the second loop remade is
# the build's to remove — `remove_intermediates` skips one whose update_status is
# still us_none, which is the same distinction as the case beside this one.
VPATH = build
GPATH = build

.INTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
