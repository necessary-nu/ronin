# GPATH says the directory the search looked in is where the target belongs, so
# one that is already current there is not remade anywhere.
VPATH = build
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
