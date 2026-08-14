# Remaking a makefile starts the read over from the beginning, and the wildcard
# on the way through runs again: the second pass finds the file the remaking
# recipe made, where the first pass found nothing.
$(file >> log,[$(wildcard *.probe)])
include gen.mk

gen.mk: ; @printf 'x\n' > made.probe; printf '# generated\n' > $@
all: ; @true
