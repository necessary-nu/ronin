# `check_dep` (remake.c) asks an intermediate whether it is NEWER than the file
# being checked, never whether it is out of date, and when it is not it walks the
# intermediate's own prerequisites against that same date. build/out.o is stale
# and out.c is still older than app, so nothing under app is newer than app and
# no recipe runs — the intermediate's own staleness is not app's business.
VPATH = build
GPATH = build

.SECONDARY: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
