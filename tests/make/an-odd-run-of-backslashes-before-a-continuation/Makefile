# It is the backslash immediately before the newline that makes the pair, and
# the ones in front of it are text: three of them reach the shell as three,
# with the newline after them.
all:
	@printf '%s' 'a\\\
	b' | od -An -c > out
