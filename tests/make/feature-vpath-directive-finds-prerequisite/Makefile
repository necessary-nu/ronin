# A vpath directive gives a search path for names matching a pattern, and the
# prerequisite the recipe is handed is the one that was found.
vpath %.in src

all: out

out: found.in ; @printf '%s\n' '$<' > $@
