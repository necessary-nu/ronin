# The clause that comes first: a `::` entry that declared no prerequisites at
# all is always out of date, and it is asked before the recipe-less overruling
# is. Such an entry is therefore never current, so it never renames — and
# having no recipe it still runs nothing, which is what makes this the cell
# that separates "out of date" from "has work".
VPATH = src
out.o::
out.o:: late
	@echo "two at=$@" >> log.txt
	@echo two >> $@
