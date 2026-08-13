# An included Makefile newer than what it is made from is already what it will
# be, so its rule does not run and the read is not started again: MAKE_RESTARTS
# stays the empty value a first read has.
all: ; @printf 'restarts=%s value=%s\n' '$(MAKE_RESTARTS)' '$(V)' > out

include gen.mk

gen.mk: gen.in ; @printf 'V := remade\n' > $@
