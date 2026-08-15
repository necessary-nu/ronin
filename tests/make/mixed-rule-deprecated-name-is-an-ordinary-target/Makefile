# The name GNU Make deprecates is not thrown away: it becomes an ordinary
# target spelt with a percent in it, carrying this rule's recipe, and asking
# for it by that spelling runs the recipe.
a %.o: ; @printf 'ran %s\n' '$@' > out
