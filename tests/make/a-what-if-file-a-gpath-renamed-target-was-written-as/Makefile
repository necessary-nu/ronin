# `-W` names the target as the Makefile wrote it, and the stamp holds through
# the rename: the switches are read before the update, so the file object the
# date lands on is the one `f_mtime` then moves into the GPATH directory. Here
# nothing is out of date without the switch, and app is remade with it.
VPATH = src
GPATH = src
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
