# A command-line assignment reaches a sub-make through MAKEFLAGS and has to go
# on reaching the one below that, which is two levels from where it was typed.
# The middle level assigns a name of its own and assigns one of these again.
all: ; @$(MAKE) -f mid.mk ALSO=middle OVER=middle
