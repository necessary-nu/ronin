# A Makefile declared .PHONY is out of date whenever it is considered, which is
# the other shape that would restart the read forever. Its rule is refused too,
# and the optional include stays unread.
all: ; @printf 'value=%s\n' '$(V)' > out

-include gen.mk

.PHONY: gen.mk
gen.mk: ; @printf 'V := phony\n' > $@
