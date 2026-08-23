# The other side of GNU Make's asymmetry: without `.ONESHELL` the same value
# goes back through the tokeniser, `./shell` is the program and `-x` its first
# argument, and both lines run. The two cases differ in nothing but the
# `.ONESHELL:` line.
SHELL = ./shell -x
all:
	echo one > one.txt
	echo two > two.txt
