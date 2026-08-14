# A prerequisite spelled `-lNAME` is not a file name. Make substitutes NAME
# into each whitespace-separated element of `.LIBPATTERNS` and searches for the
# results, and the prerequisite becomes what it found — which is what `$<` and
# `$^` are then worth.
.LIBPATTERNS = mtest_%.a
all: -lfoo ; @printf '%s\n' 'from $< all=$^' > out
