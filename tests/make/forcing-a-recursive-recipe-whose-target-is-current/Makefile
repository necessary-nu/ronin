# `-B` makes every target with a recipe out of date, and a recursive recipe is
# a recipe. The compilation decides whether such a recipe has to run before it
# reads any child Makefile of it, so that decision has to be taken with the
# switch in hand: without it the wrapper is judged current against a target
# that is already there, no child is composed, and the edge staged to ask the
# question is left carrying the question.
pre.out:
	@echo before > pre.out
	@$(MAKE) -f sub.mk sub
	@echo after >> pre.out
