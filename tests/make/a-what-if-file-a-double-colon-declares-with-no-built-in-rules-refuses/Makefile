# The control for the case beside this one, and it changes nothing about the
# tree: out.c is still there. `-r` takes the built-in rules away, so the search
# offered the appended entry finds nothing, and the refusal is back.
all: app
app: out
	@echo made > $@
out:: a
	@echo remade > $@
out:: b
	@echo again >> $@
