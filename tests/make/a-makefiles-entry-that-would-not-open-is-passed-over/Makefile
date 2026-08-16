# MAKEFILES entries are read with RM_DONTCARE (read.c:204), and GNU Make never
# looks at errno after them the way the -f loop does — so a name that would not
# open is as quiet as a name nothing is at, and the goals build. `afile` is an
# ordinary file, so `afile/one.mk` fails to open with ENOTDIR.
all: ; @printf 'all\n' > ran
