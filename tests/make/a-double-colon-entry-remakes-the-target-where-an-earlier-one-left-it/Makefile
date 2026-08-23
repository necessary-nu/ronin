# GNU Make walks a `::` chain entry by entry, and `update_file_1` (remake.c)
# ends an entry it does not have to remake with
#
#     while (file) { file->name = file->hname; file = file->prev; }
#
# which renames that entry and every entry AFTER it. So the found path, once an
# entry is current, is the answer for the rest of the chain: `keep` is current
# and commits the rename, and `late` then remakes the file where `keep` left it
# — under `src`, not here.
VPATH = src
out.o:: keep
	@echo "one at=$@" >> log.txt
	@echo one >> $@
out.o:: late
	@echo "two at=$@" >> log.txt
	@echo two >> $@
