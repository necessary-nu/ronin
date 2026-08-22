# -B forces every target with a recipe, and -o is read before it: update_file_1
# returns on file->updated ahead of the always_make arm, so the named file is
# the one target -B does not reach.
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
