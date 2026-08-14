# Without GPATH the directory search only says where an older copy is lying:
# a target that has to be remade is remade here, under the name as written.
VPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
