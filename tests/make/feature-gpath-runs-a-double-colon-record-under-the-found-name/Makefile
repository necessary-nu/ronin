# Each `::` record was filed under the name the makefile wrote, and the rename
# moves the target rather than the records. Both run, both make the found path,
# and the stem each reads is the renamed name with a known suffix taken off.
VPATH = build
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o:: out.c ; @printf 'one [%s] from [%s] stem [%s]\n' '$@' '$<' '$*' > one
out.o:: out.h ; @printf 'two [%s] from [%s] stem [%s]\n' '$@' '$<' '$*' > two
