# A double-colon rule with no recipe still has its own prerequisites brought up
# to date, but it contributes nothing for the chain to run, and being out of
# date does not lend that verdict to the rule that does have a recipe.
out:: made.in
out:: kept.in
	@printf '%s\n' recipe >> log

made.in:
	@printf '%s\n' made >> log
	@touch made.in
