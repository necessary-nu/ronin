# An entry of a `::` chain that wrote no recipe still answers for the chain's
# name. GNU Make's `update_file_1` ends a target it does not have to remake by
# renaming that entry and every entry after it, whether or not there was
# anything to run — so the entry below, having nothing to do, hands the found
# path to the entry after it, which remakes the file down the search path.
VPATH = src
out.o:: keep
out.o:: late
	@echo "two at=$@" >> log.txt
	@echo two >> $@
