# An expansion-time error only fires when the recipe holding it is expanded,
# and a target that is already up to date never expands its own.
out: in ; @cp in out $(error this recipe was never expanded)
