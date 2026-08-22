# The same two bytes, outside quotes, where the shell removes them: this is
# why nearly every continued recipe agrees whatever the reader did with them,
# and it is the control for the ones that do not.
all:
	@printf '%s' a\
	b | od -An -c > out
