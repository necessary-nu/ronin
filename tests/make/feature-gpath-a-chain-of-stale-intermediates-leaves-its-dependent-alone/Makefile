# `check_dep` recurses with the same date it was given, so a whole chain of
# intermediates is weighed against the dependent and never against each other.
VPATH = build
GPATH = build

.SECONDARY: out.o mid.o

all: app

app: mid.o ; @cat $< > $@

mid.o: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
