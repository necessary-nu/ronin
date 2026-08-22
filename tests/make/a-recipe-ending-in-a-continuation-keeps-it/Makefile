# The last line of a recipe may end in a continuation with nothing to continue
# into. GNU Make writes the pair out all the same and the shell discards it, so
# the trailing bytes are not the reader's to tidy away -- taking the newline off
# and leaving the backslash would hand the shell an escape with nothing after
# it.
all:
	@printf '%s' 'ab' | od -An -c > out\
