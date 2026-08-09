# A bare `export` exports everything from then on, and a named unexport is
# still the last word on the name it gives.
one = 1
two = 2

export
unexport two

all: ; @printf 'one=%s two=%s\n' "$$one" "$$two" > out
