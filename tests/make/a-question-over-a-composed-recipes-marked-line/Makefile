# A `+` line runs even where the build is standing in for its recipe: GNU Make
# touches the target and runs the marked lines. This recipe composes a `$(MAKE)`,
# so it is cut into segments and each segment reaches one shell as an assembled
# script — one launch, with one answer to "does this run anyway", and the answer
# it was given is no. So the marked lines do not run.
#
# RECORDED DIVERGENCE. See the `divergence` file beside this one.
goal:
	+@echo marked-before > before.out
	@$(MAKE) --no-print-directory -C sub child
	+@echo marked-after > after.out
	@echo plain > plain.out
