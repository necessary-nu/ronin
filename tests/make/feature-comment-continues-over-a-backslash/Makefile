# A comment runs to the end of the line, so a semicolon in one is not a recipe,
# and a backslash at the end of a comment carries it on to swallow the line
# after it.
all: # this ; is only a comment, and it goes on \
	@printf '%s\n' swallowed > wrong
	@printf '%s\n' kept > out
