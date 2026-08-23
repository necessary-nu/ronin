# The same, over a chain of one. GNU Make appends the fresh entry to a chain of
# one as readily as to a chain of three, so the switch is inert here too — and
# this is the shape a compiled graph cannot recognise for itself, because a lone
# `::` record keeps the single-node shape an ordinary rule has.
all: app
app: out
	@echo made > $@
out:: a
	@echo remade > $@
