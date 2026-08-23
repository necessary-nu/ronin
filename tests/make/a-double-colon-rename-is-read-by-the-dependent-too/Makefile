# What the committed rename is read by, beyond the entry that writes: the
# dependent spells the prerequisite the way the build settled it, because GNU
# Make reads `$<` off the file object whose name `update_file_1` has already
# changed. `$@` in the entry, `$<` and `$^` in the dependent, and the three
# halves the name splits into all read the found path.
VPATH = src
all: out.o
	@echo "all first=$< every=$^" >> log.txt
out.o:: keep
	@true
out.o:: late
	@echo "two at=$@ dir=$(@D) file=$(@F)" >> log.txt
	@echo two >> $@
