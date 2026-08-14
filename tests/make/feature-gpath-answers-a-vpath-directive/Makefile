# The directive form of the search reaches GPATH the same way the variable does.
vpath %.o build
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
