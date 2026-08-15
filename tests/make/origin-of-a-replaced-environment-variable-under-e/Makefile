# Writing to the name is what asks `-e` to decide, and the answer is the
# environment: the assignment is declined and the origin says so.
LC_ALL = from-file
all: ; @printf '%s\n' '$(origin LC_ALL)' '$(LC_ALL)' > out
