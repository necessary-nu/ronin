# Which makefile the recipe-less rule was written in does not matter: every
# rule joins one list as it is read.
%.gen: %.src ; @printf '%s\n' built > $@
include cancel.mk

all: foo.gen
