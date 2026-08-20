# A tab ends the name, so this is a call -- and being a call it is judged like
# one: the close never arrives and the read that reaches it refuses. Read as a
# variable name instead, the close paren belonging to the argument would end an
# unmatched-parentheses reference, the build would go through, and the shell
# inside the arguments would never run either way.
X := $(subst	$(shell touch inside-the-call),b,c

all: ; @echo built > out
