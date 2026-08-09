# What the recipe's shell sees is only what was exported: by name, by an export
# that also assigns, and never what was withdrawn again.
plain = p
exported = e
withdrawn = w

export exported
export assigned = a
unexport withdrawn

all: ; @printf 'plain=%s exported=%s assigned=%s withdrawn=%s\n' "$$plain" "$$exported" "$$assigned" "$$withdrawn" > out
