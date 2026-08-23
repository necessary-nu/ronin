# The other spelling of the same target, and `-o` reaches nothing with it. A
# name that is not a target when the switch is read gets a file object of its
# own; the rename then merges the two and `rehash_file` keeps the LATER of the
# dates (file.c), so an asserted OLD one loses to the date the search has just
# taken. The build runs as though the switch were not there.
VPATH = src
GPATH = src
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
