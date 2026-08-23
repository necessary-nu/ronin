# A `::` chain inside a recursive child. The child's recipe runs where the
# child's Makefile was read, so GNU Make answers `$?` with the name that
# Makefile wrote — never one qualified from the parent's directory, which
# would point at a file that is not there from where the recipe stands.
all:
	@$(MAKE) -C sub
