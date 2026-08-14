# A hit in the working directory ends the search there and then, so it beats a
# vpath hit an EARLIER element already recorded. The vpath answers are only
# ever provisional.
vpath %.a vd
.LIBPATTERNS = first_%.a second_%.b
all: -lfoo ; @printf '%s\n' 'from $<' > out
