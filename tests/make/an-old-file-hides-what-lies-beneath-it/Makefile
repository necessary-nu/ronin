# update_file_1 returns on file->updated before it looks at a prerequisite, so
# an -o name stops the walk: out.o is stale and is not remade either, because
# nothing ever considers it. app does not exist and is still not made.
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
