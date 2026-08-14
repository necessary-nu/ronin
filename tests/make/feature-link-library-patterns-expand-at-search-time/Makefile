# `.LIBPATTERNS` is expanded when the search runs rather than read as text, so
# a recursive value follows an assignment made anywhere in the read — here one
# written below the rule that names the library.
.LIBPATTERNS = $(PAT)
all: -lfoo ; @printf '%s\n' 'from $<' > out
PAT = mtest_%.a
