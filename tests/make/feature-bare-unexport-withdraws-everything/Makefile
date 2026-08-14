# `unexport` with no names is the same switch thrown the other way, and the
# last of the two to be read is the one that counts.
one = 1
export
unexport
export
two = 2
unexport
all: ; @printf 'one=[%s] two=[%s]\n' "$$one" "$$two" > out
