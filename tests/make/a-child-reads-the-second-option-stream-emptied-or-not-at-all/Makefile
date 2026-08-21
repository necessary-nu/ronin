# What a child is handed, which is upstream's own question about this name
# (tests/scripts/variables/GNUMAKEFLAGS): the switches must reach the child
# once, through MAKEFLAGS, and the second stream must reach it emptied so it
# cannot decode them a second time.
#
# The child reads the ORIGIN rather than the bytes, because the two ways of
# arriving with nothing are different things and only the origin tells them
# apart. `environment` says the name was handed down empty; `default` would say
# it was never handed down at all, which is what the sibling case measures.
all:
	@$(MAKE) --no-print-directory -f child.mk
