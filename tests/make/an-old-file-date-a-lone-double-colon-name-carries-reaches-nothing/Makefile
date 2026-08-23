# `-o` over the same chain of one, and the cell where the switch has something
# to lose: `out` is NEWER than `app`, so without the switch `app` is remade, and
# with a stamp that reached the target it would not be. The stamp reaches
# nothing, so `app` is remade either way.
all: app
app: out
	@echo made > $@
out:: a
	@echo remade > $@
