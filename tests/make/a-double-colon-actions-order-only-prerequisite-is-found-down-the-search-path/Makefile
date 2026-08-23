# The order-only half of the same search. `oo.h` is only under `src`, and `$|`
# names it there; the ordinary prerequisite of the second record is here and is
# named as written. Both lists of every action go through the search.
VPATH = src
all: out.o
out.o:: | oo.h
	@echo "one |<$|>" >> log
	@echo o > $@
out.o:: dep
	@echo "two <$<>" >> log
	@echo o2 >> $@
