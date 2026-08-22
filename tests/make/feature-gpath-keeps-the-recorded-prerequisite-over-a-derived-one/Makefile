# Re-applying the target pattern to the renamed name would find build/out.c and
# build from it. The record decides instead, so the file the search directory
# happens to hold is not read at all.
VPATH = build
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: %.o: %.c ; @printf 'made [%s] from [%s] stem [%s]\n' '$@' '$<' '$*' > $@
