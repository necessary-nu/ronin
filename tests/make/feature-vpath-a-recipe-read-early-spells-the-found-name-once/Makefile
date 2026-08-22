# A recipe naming `$?` is one the compiler has to read before the build, so the
# prerequisite names in it are settled then. `$?` is a value the scheduler binds
# and takes the found spelling; `$^`, `$<` and `$+` were written into the text
# and keep the name as it was written.
VPATH = build

all: out.o ; @printf 'whole [%s] new [%s] first [%s]\n' '$^' '$?' '$<' > seen

out.o: out.c ; @printf 'made\n' > $@
