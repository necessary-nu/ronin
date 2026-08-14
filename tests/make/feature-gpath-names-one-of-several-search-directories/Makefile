# Each name is answered by the directory it was found in: kept where GPATH
# names that directory, remade here where it does not.
VPATH = alpha beta
GPATH = beta

all: one.o two.o ; @printf 'all sees %s\n' '$^' > seen

one.o: one.c ; @printf '%s from %s\n' '$@' '$<' > $@
two.o: two.c ; @printf '%s from %s\n' '$@' '$<' > $@
