# GPATH answers for the directory the search found the target in and for no
# other, so naming a different one changes nothing.
VPATH = build
GPATH = other

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
