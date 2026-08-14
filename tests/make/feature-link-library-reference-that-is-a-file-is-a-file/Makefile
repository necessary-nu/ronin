# The search is the last resort and not the first. A file actually named
# `-lfoo` is what the prerequisite means, however well `.LIBPATTERNS` would
# have resolved it.
.LIBPATTERNS = mtest_%.a
all: -lfoo ; @printf '%s\n' 'from $<' > out
