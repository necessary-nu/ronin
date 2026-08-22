# Each continuation is read on its own, so a line continued twice reaches the
# shell with two pairs in it and neither of them folded into the other.
all:
	@printf '%s' 'a\
	b\
	c' | od -An -c > out
