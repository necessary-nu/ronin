# `-t` with the record's HEAD out of date, which is the other half of
# `a-touch-over-a-double-colon-chain-follows-the-rename`. The touch is each
# entry's own reading and never the dependent's: the head brings the name as
# written up to date, the recipe-less entry after it is current and renames the
# rest of the record, and the third entry then touches the found path. Two files
# come up to date in one run, and which one each entry chose is the whole cell.
VPATH = src
all: out.o
	@echo "first=$<" >> log.txt
out.o:: late
	@echo one >> $@
out.o:: keep
out.o:: late
	@echo three >> $@
