# `foo` can be linked two ways here: `%: %.o` and `%: %.c` both match it with
# the stem `foo`, and only `foo.c` is on disk. GNU Make still picks `%: %.o`,
# because `foo: foo.o` already asks for `foo.o` — a prerequisite the target
# itself names "ought to exist" whichever rule the search settles on, so the
# candidate that wants it invents nothing and applies on the strict pass, ahead
# of the later candidate whose prerequisite merely happens to be there.
#
# Read `foo.o` as an invented intermediate instead and `%: %.c` wins, which
# puts the source on the link line: `$^` becomes `foo.c foo.o` and the linker
# is handed a `.c` file it was never meant to see.
#
# The order the two candidates are tried in is `.SUFFIXES`', not the order the
# rules are written in — `.c:` is written first below and still loses.
.SUFFIXES:
.SUFFIXES: .o .c

foo: foo.o

.c.o: ; @printf 'compile %s <- [%s]\n' '$@' '$^' > compile.out ; : > $@
.c: ; @printf 'link.c %s <- [%s]\n' '$@' '$^' > link.out
.o: ; @printf 'link.o %s <- [%s]\n' '$@' '$^' > link.out
