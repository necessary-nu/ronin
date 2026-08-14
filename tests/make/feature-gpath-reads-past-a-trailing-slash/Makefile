# A search path is a list of directories rather than of strings, so a trailing
# slash is not part of the name GPATH is compared against.
VPATH = build
GPATH = build/

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
