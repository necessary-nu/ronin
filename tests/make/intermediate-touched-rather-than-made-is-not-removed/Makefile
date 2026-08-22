# `remove_intermediates` (file.c) punts on the whole run under -t, so an
# intermediate the touch brought into existence outlives the build that made it.
.INTERMEDIATE: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
