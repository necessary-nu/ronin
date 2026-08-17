# A wildcard inside the member position is matched against the archive's own
# index, not against the filesystem: GNU Make's `ar_glob` (reference/gnumake/
# src/ar.c) walks the members `ar_scan` finds and re-emits one `lib.a(...)`
# name per match, sorted.
#
# Both halves of that are asserted here. `q.o` is on disk beside the archive
# and is not filed in it, so a filesystem glob would have matched it and the
# index does not. The index is written in the order c.o, a.o, b.o, so a reader
# that returned the members as it found them would not produce this line.
all: libx.a(*.o)
	@echo "[$^]" > log
