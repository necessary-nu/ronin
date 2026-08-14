# The same makefile with the two vpath directives swapped, which is what makes
# the previous case evidence rather than a coincidence: the answer follows the
# directive order and nothing else about it changed.
vpath %.a vd2
vpath %.b vd1
.LIBPATTERNS = first_%.a second_%.b
all: -lfoo ; @printf '%s\n' 'from $<' > out
