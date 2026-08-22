# A `::` entry that is also a static pattern rule records its own prerequisite
# per target, so the rename carries two records rather than one and each entry
# reads the source its own prerequisite pattern derived.
VPATH = build
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o:: %.o: %.c ; @printf 'one [%s] from [%s] stem [%s]\n' '$@' '$<' '$*' > one
out.o:: %.o: %.h ; @printf 'two [%s] from [%s] stem [%s]\n' '$@' '$<' '$*' > two
