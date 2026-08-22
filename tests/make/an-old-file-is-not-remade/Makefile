# -o FILE says the file is very old and must not be remade. GNU Make stamps
# OLD_MTIME on it and marks it updated (main.c), so its own recipe never runs
# however stale the file is.
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
