# A `+` line runs even where the build is standing in for its recipe: GNU Make
# touches the target and runs the marked lines. This recipe composes a `$(MAKE)`,
# so it is cut into segments around the invocation — the line ahead of it, and
# the recipe's own trailing lines. Each segment carries its per-line launches,
# so the `+` marker is read per line: under `-t` the marked lines run and the
# rest are stood in for, and `goal` is touched rather than made. The child is
# reached for real, because the segment ahead of it is compiler input.
goal:
	+@echo marked-before > before.out
	@$(MAKE) --no-print-directory -C sub child
	+@echo marked-after > after.out
	@echo plain > plain.out
