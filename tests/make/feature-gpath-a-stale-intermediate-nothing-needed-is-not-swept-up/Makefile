# The same makefile with app newer than everything: no recipe ran, so
# `remove_intermediates` reaches an intermediate whose update_status is us_none
# and passes over it.
VPATH = build
GPATH = build

.INTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
