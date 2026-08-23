# Which entry's rename `$<`, `$^` and `$?` in a dependent read, which is one
# question and not three: GNU Make reads all of them off the same file objects,
# and the object a dependent holds is the entry the record was filed under. The
# head is remade here, so every one of them is the name as written, and the
# ordinary prerequisite beside the chain is the control that says the reading is
# per prerequisite rather than per list.
VPATH = src
all: out.o other
	@echo "first=$< every=$^ new=$?" >> log.txt
out.o:: late
	@echo one >> $@
out.o:: keep
	@echo two >> $@
other:
	@echo other >> $@
