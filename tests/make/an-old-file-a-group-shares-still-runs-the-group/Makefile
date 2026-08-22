# -o is answered per name. One member of a grouped record being settled is no
# reason to spare the record: b is still out of date, the recipe runs, and it
# writes a as well.
all: a b
a b &: src
	@echo made > a
	@echo made > b
