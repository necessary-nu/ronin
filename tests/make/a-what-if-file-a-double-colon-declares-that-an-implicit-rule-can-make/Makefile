# The entry `-W` appends to a `::` chain has no recipe, so `update_file_1` offers
# it the implicit rule search like any other file with none — and here the
# built-in `%: %.c` matches, because out.c is lying beside it. The entry has a
# rule and prerequisites of its own, so nothing is complained about and the
# build runs. The same tree with no out.c is the case beside this one.
all: app
app: out
	@echo made > $@
out:: a
	@echo remade > $@
out:: b
	@echo again >> $@
