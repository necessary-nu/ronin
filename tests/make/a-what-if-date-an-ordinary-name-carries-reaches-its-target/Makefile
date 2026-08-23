# The control, and the thing the two cases beside it must not break: the same
# tree with a single-colon rule. `-W out` reaches the target, `app` reads a
# prerequisite newer than itself, and `app` is remade.
all: app
app: out
	@echo made > $@
out: a
	@echo remade > $@
