# A `::` recipe with no prerequisites runs whenever its target is considered,
# so a Makefile written that way would be remade on every read and the read
# would never end. Make refuses to remake it at all rather than start over
# forever, and the optional include stays unread.
all: ; @printf 'value=%s\n' '$(V)' > out

-include gen.mk

gen.mk:: ; @printf 'V := double-colon\n' > $@
