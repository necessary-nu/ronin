# Double quotes are the other control. The shell removes a backslash-newline
# inside them, so the pair arriving intact makes no difference to what the
# command finally is.
all:
	@printf '%s' "a\
	b" | od -An -c > out
