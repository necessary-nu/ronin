# The value of a target-specific assignment runs to the end of the line, so a
# semicolon in it is part of the value and does not begin a recipe.
all: FOO = ; ok

all: ; @printf '%s\n' '$(FOO)' > out
