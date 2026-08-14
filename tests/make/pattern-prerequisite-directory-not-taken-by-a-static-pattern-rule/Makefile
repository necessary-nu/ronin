# The directory split lives in the implicit search alone. A static pattern rule
# substitutes the whole stem where the `%` stands, directories and all, so the
# same rule text reads a different name depending on how it was reached.
all: lib/bye.x

lib/bye.x: %.x: 6% ; @printf '%s\n' '$^' > log; printf 'stem=%s\n' '$*' >> log; touch $@
