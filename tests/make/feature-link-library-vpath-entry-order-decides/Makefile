# Between two answers that both came from a vpath, the earliest vpath entry
# wins whichever element reached it — the element order does not get a say.
# `second_%.b` is the later element and takes it, because `vpath %.b vd1` is
# the earlier directive.
vpath %.b vd1
vpath %.a vd2
.LIBPATTERNS = first_%.a second_%.b
all: -lfoo ; @printf '%s\n' 'from $<' > out
