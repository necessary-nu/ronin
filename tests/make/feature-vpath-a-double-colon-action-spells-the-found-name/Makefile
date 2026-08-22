# Each `::` record is an action of its own and its recipe is read while the
# graph is built, so it carries the same references a `$?` recipe does.
VPATH = build

all:: out.o ; @printf 'one [%s] first [%s]\n' '$^' '$<' > one

all:: out.o ; @printf 'two [%s] first [%s]\n' '$^' '$<' > two

out.o: out.c ; @printf 'made\n' > $@
