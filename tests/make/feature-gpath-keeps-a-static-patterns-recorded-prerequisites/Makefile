# A static pattern rule's prerequisites are worked out when the rule is read,
# against the name as written, and GPATH then renames the one object that
# carries them. So the target is made at the found path from the prerequisite
# the written name derived, and the stem is the written name's.
VPATH = build
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: %.o: %.c ; @printf 'made [%s] from [%s] stem [%s]\n' '$@' '$<' '$*' > $@
