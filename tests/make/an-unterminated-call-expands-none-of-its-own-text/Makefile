# An unterminated call is found by counting parens over text that has not been
# expanded, so nothing inside it runs -- while everything the expander already
# walked past on its way there has. The two files this leaves say which half is
# which.
BOTH := $(shell touch before-the-call)$(subst $(shell touch inside-the-call),b,c

all: ; @echo built > out
