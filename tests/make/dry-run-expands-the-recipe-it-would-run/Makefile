# A dry run expands the recipes it would run, because the text is what it
# prints, so the expansion's own effects happen while nothing is built.
all: ; @printf 'ran\n' > out $(shell printf 'expanded\n' > expansion.log)
.PHONY: all
