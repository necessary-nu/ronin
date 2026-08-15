.PHONY: all /x.o /y
all: /x.o
/x.o: /y
	@printf '[%s]\n' '$@' '$(@D)' '$(@F)' '$<' '$(<D)' '$(<F)' '$(^F)' > out
/y: ; @:
