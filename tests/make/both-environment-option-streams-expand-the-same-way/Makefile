# The expansion is uniform across both streams: GNUMAKEFLAGS is expanded
# exactly as MAKEFLAGS is, immediately before it, because both reach the same
# decoder. So `GNUMAKEFLAGS=$(subst X,-,Xk)` decodes to `-k` and is then folded
# into MAKEFLAGS and the name emptied, exactly as a literal `-k` would be.
$(file > read,gmf=[$(GNUMAKEFLAGS)] mf=[$(MAKEFLAGS)])

all: bad good

bad:
	@exit 1

good:
	@printf 'the second stream expanded to -k\n' > kept-going
