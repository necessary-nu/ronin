# The refusal is about the shape that never settles, not about `::`. A double
# colon rule that names a prerequisite is out of date only while the file is,
# so the Makefile is remade once and the read starts again.
all: ; @printf 'restarts=%s value=%s\n' '$(MAKE_RESTARTS)' '$(V)' > out

-include gen.mk

gen.mk:: gen.in ; @printf 'V := double-colon\n' > $@
