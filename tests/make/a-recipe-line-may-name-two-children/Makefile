# A recipe line that is nothing but invocations joined by `&&` names each of
# them. `&&` runs the next only when the last one won, and in that order,
# which is what one child ordered after another does, so the line and the two
# children describe the same build.
#
# The second line writes the same thing through wrappers a shell reaches
# straight through: a subshell and a brace group hold one command each, and
# which child a line names does not depend on how it was written.
all:
	@$(MAKE) --no-print-directory -C a && $(MAKE) --no-print-directory -C b
	@(cd c && $(MAKE) --no-print-directory) && { cd d && $(MAKE) --no-print-directory; }
