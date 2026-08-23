# Three `::` records, three actions, three separate searches. What the search
# found is what the recipe is handed — `$<` and `$^` name the found path — and
# an action whose prerequisite is here keeps the name as written. A search that
# reached only the first record, or that answered once for the whole chain,
# writes a different log.
VPATH = src
all: out.o
out.o:: a.c
	@echo "one <$<> ^<$^>" >> log
	@echo o > $@
out.o:: b.c
	@echo "two <$<> ^<$^>" >> log
	@echo o2 >> $@
out.o:: c.c
	@echo "three <$<> ^<$^>" >> log
	@echo o3 >> $@
