# Suffix rules become pattern rules once every makefile has been read, and one
# whose name a written pattern rule already holds is thrown away. So the
# recipe-less rule withdraws the suffix rule too, whichever came first.
%.gen: %.src
.SUFFIXES: .src .gen
.src.gen: ; @printf '%s\n' built > $@

all: foo.gen
