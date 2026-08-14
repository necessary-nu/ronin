# With a library reachable through more than one element, the earliest element
# that reaches one in the working directory decides — and it ends the search
# where it stands rather than weighing the rest.
.LIBPATTERNS = second_%.a first_%.b
all: -lfoo ; @printf '%s\n' 'from $<' > out
