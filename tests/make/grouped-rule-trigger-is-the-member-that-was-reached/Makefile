# The control for the pattern form beside it. An explicit `&:' group is one
# decision over all its targets, so reaching any of them runs the recipe for
# the name that was reached — a.c here, though a.c is the current one and
# a.h is the one missing.
all: a.c a.h
a.c a.h &: a.in ; echo 'at=$@' >> log ; cat $< > a.c ; cat $< > a.h
