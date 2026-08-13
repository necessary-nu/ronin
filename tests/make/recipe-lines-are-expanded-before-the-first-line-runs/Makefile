# The whole recipe is expanded in one go, immediately before its first line
# runs, so a later line cannot see what an earlier line of the same recipe did.
all:
	@printf 'made\n' > made
	@printf '[%s]\n' '$(wildcard made)' > out
.PHONY: all
