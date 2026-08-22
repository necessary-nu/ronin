# A target found where the search looked and current there is spelt that way in
# every list a recipe reads it from, `$?` included — GNU Make reads them all off
# one file object, whose name update_file_1 has already settled.
VPATH = build

all: out.o ; @printf 'new [%s]\n' '$?' > seen

out.o: out.c ; @printf 'made\n' > $@
