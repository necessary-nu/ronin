# The third of the three words a refused `.SHELLFLAGS` reaches the launch as is
# not the flags text as written. The recursion that produced it is the one-shell
# branch, whose shell there is the default `/bin/sh` and so is Bourne-compatible,
# and that branch takes the blanks and `[@+-]` off the front of every line of the
# text it is about to hand over — a `SHELL` Make does not recognise is the only
# reason it ever leaves them.
#
# So the leading `-` of `-e` is gone by the time the shell sees it, and 4.4.1
# hands `./shell` the word `e "x" -c`.
.ONESHELL:
SHELL = ./shell
.SHELLFLAGS = -e "x" -c
all:
	echo hello
