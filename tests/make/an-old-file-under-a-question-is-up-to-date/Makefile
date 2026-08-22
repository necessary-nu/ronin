# -q exits 1 while anything is out of date. -o app settles the only thing
# that was, so the question answers zero and still writes nothing.
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
