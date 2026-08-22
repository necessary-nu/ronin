# OLD_MTIME is 2, one below the smallest ordinary timestamp, so an -o file is
# older than every real file and a dependent that already exists is newer than
# it. Without the switch app is remade; with it nothing is.
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
