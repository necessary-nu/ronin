# GNU Make expands a recipe when it is about to run it. This target is already
# newer than its prerequisite, so the recipe is never expanded and the shell
# call inside it never runs.
out: in ; @cp in out $(shell printf 'ran\n' > expansion.log)
