# A wildcard answered while reading does not settle what a later one answers.
# The recipe expanded after the first recipe ran finds the file it made, though
# the same pattern found nothing at all while the makefile was read.
reading := $(wildcard *.probe)

all: second
first: ; @printf 'x\n' > late.probe
second: first ; @printf '[%s] [%s]\n' '$(reading)' '$(wildcard *.probe)' > out
.PHONY: all first second
