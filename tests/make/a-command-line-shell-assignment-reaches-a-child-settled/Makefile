# `MAKEOVERRIDES` carries what a command-line assignment SETTLED to, not the
# operator it was written with, so the child is handed `FOO=hi` and does not run
# the command again. `counted` is what says so: one line for the whole tree.
all: ; @$(MAKE) --no-print-directory -f child.mk
