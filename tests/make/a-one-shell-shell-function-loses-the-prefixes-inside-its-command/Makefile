# The one-shell branch takes the blanks and `[@+-]` off the front of every
# logical line of the text it is about to hand a Bourne-compatible shell, and a
# `$(shell)` expanded under `.ONESHELL:` is such a text. So the `@` is Make's to
# eat and the command runs; read as an ordinary command line the same `@echo`
# is a program nothing can find.
#
# WHERE THE LINE STANDS DECIDES IT. `read.c:1925` latches the global the moment
# the target is recorded, so the `$(shell)` above the `.ONESHELL:` line is read
# as one command line and the one below it as a script. Both write their file
# either way — the failing `@echo` still opens its redirect — so what separates
# them is what the file HOLDS.
BEFORE := $(shell @echo up > up.txt ; cat up.txt)
.ONESHELL:
AFTER := $(shell @echo down > down.txt ; cat down.txt)
all:
	@printf '%s' 'before=<$(BEFORE)> after=<$(AFTER)>' > out
