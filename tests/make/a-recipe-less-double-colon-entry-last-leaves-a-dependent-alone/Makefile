# The same reading with the renaming entry carrying no recipe, which is the
# shape that reaches it without any entry after it to hand the found path to.
# The first entry is remade and keeps the name as written; the second has
# nothing to run, is found current and renames itself alone. A dependent still
# holds the first entry's name.
VPATH = src
all: out.o
	@echo "all first=$<" >> log.txt
out.o:: late
	@echo "one at=$@" >> log.txt
	@echo one >> $@
out.o:: keep
