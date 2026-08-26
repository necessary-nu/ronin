# The same rule where the recipe is empty as written rather than after
# expansion: `b: c;` declares a recipe of no lines, and a declared recipe is
# what `-t` stands in for.
.SUFFIXES:
a: b
	@echo made > $@
b: c;
