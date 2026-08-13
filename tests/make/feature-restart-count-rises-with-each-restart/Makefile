# MAKE_RESTARTS counts starts rather than remade files: the first read has no
# such variable, and each start over adds one. A generated Makefile that itself
# includes a generated Makefile takes two.
all: ; @printf 'restarts=%s first=%s second=%s\n' '$(MAKE_RESTARTS)' '$(A)' '$(B)' > out

-include one.mk

one.mk: ; @printf -- '-include two.mk\nA := one\n' > $@
two.mk: ; @printf 'B := two\n' > $@
