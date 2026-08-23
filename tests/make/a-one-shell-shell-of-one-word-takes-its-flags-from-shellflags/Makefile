# The control beside the case above, and where the flags legitimately live:
# `$(SHELL)` names one path, `.SHELLFLAGS` carries the words that go with it,
# and the recipe runs. A launch that split the shell value would pass this too;
# one that stopped splitting it and did not keep this working would not.
.ONESHELL:
SHELL = ./shell
.SHELLFLAGS = -x -c
all:
	echo one > one.txt
	echo two > two.txt
