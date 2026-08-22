# The directory search preserves OLD_MTIME rather than stating the file it
# found (vpath.c), so -o names the file as the makefile wrote it and the answer
# survives the search. app stays as it was.
VPATH = src
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
