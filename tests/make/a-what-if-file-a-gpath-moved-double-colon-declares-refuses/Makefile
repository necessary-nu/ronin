# The refusal reaching a target the search moved. `-W out` names the record as
# the Makefile wrote it, and the entry GNU Make appends to the `::` chain is
# carried into the GPATH directory with the rest of the chain — so the name the
# complaint carries is the one it was moved to. Nothing here can make it: there
# is no out.c for a built-in rule to find.
VPATH = src
GPATH = src
all: app
app: out
	@echo made > $@
out:: a
	@echo remade > $@
out:: b
	@echo again >> $@
