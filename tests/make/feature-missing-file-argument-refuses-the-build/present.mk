# A file named by -f that is not there is a target make cannot reach, so the
# run is refused rather than continued on the files that were readable.
all: ; @printf '%s\n' built > out
