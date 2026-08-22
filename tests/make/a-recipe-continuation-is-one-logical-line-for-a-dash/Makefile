# A continued line is ONE command line: the `-` written in front of it forgives
# the failure of the whole thing, and the recipe carries on to the line after
# it. What the forgiven line wrote is the evidence that the pair reached the
# shell as well.
all:
	-@printf '%s' 'a\
	b' > out; exit 1
	@printf 'after\n' > after
