# A goal is a file like any other, so a goal found in a GPATH directory is
# remade there rather than here.
VPATH = build
GPATH = build

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
