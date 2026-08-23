# `-t` stands in for the recipes, and GNU Make renames the file before it
# reaches the touch — `touch_file` (remake.c) is handed `file->name` already
# changed. So the switch brings the FOUND path up to date and leaves nothing
# here, exactly where the recipes it stood in for would have written.
VPATH = src
out.o:: keep
	@echo one >> $@
out.o:: late
	@echo two >> $@
