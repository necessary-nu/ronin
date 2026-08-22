# main stamps the -o files and then the -W files over them (main.c:2312,
# main.c:2325), so a name given to both is new whichever order the words were
# written in. app is remade because out.o reads as newer than it.
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
