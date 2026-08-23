# The same clause with a prerequisite that can never stop being out of date. A
# phony is always remade, so an order-only one gives the sharpest reading of
# `if (! d->ignore_mtime)`: it runs, it is never current, and the entry above
# renames the chain regardless.
VPATH = src
out.o:: | ph
out.o:: late
	@echo "two at=$@" >> log.txt
	@echo two >> $@
.PHONY: ph
ph:
	@echo "ran ph" >> log.txt
