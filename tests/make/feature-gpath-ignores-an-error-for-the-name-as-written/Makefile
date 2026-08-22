# `.IGNORE` is a bit on the file object, and `GPATH` moves the object rather
# than looking the name up again: `rehash_file` (file.c) reinserts it under the
# found path with everything it carries. So the failure at build/out.o is the
# one out.o was forgiven, and the build carries on.
VPATH = build
GPATH = build

.IGNORE: out.o

all: out.o ; @printf 'all ran\n' > seen

out.o: out.c ; @printf 'made\n' > $@; exit 1
