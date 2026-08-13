# An included Makefile older than what it is made from is brought up to date
# before the Makefile means what it will mean, and the read then starts again
# on the text that was written.
all: ; @printf 'restarts=%s value=%s\n' '$(MAKE_RESTARTS)' '$(V)' > out

include gen.mk

gen.mk: gen.in ; @printf 'V := remade\n' > $@
