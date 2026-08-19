# zsh builds its dynamically loaded modules with `.c.$(OBJ):` where OBJ is
# `.o`, so the rule is `.c..o:` and `.SUFFIXES` names `..o`. GNU splits the
# name against the suffixes it was told about; Ronin counts dots.
.SUFFIXES:
.SUFFIXES: .c ..o
.c..o:
	@echo "dyn from $<" > $@
all: foo..o
