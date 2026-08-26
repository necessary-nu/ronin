# The boundary the case beside this one sits against: `b` has prerequisites and
# no recipe, so there is nothing for the touch to stand in for. "According to
# POSIX, -t doesn't affect targets with no cmds" (remake.c).
.SUFFIXES:
a: b
	@echo made > $@
b: c
