# A composed child asks for a target the enclosing unit already made in this
# run, whose recipe writes no file of that name. GNU Make's child process stats
# `all`, finds nothing on the ground, and runs the recipe a second time. A
# compiler that reads "has a recipe and is not .PHONY" as "writes the file"
# hands the child the enclosing unit's node, drops the child's own rule, and the
# second run is lost with no diagnostic at all.
recur: all
	@$(MAKE) --no-print-directory -f Makefile all

all:
	@echo RAN >> ran.out
