# Every `::` record is a rule of its own, and GNU Make searches for the
# prerequisites of each one exactly as it searches for an ordinary rule's:
# `out.c` is not here, `src/out.c` is, and the entry is up to date against it.
# The target is searched for too, and found at `src/out.o`, so there is nothing
# to do at all. A search that did not reach the actions a `::` record compiles
# to would refuse the build outright.
VPATH = src
all: app
app: out.o
	@echo made > $@
out.o:: out.c
	@echo remade > $@
out.o:: dep
	@echo again >> $@
