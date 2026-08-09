# A vpath directive naming a pattern and no directories withdraws that
# pattern's search path, so the prerequisite is no longer found.
vpath %.in src
vpath %.in

all: found.in ; @printf '%s\n' '$<' > out
