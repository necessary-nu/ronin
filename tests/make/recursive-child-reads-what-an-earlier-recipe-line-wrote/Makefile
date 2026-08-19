# A recipe's lines run where the recipe wrote them, and the Make a $(MAKE) line
# starts reads the disk the lines ahead of it left. GNU Make gets that by
# starting the child after the line; anything that reads the child's Makefile
# instead of starting a process has to have run the line before it reads.
#
# vim's top-level Makefile in miniature. The first line puts the configuration
# the child includes in place, and without it the child's Makefile cannot be
# read at all — so this is the shape where the ordering is not cosmetic.
all:
	@if test ! -f sub/config.mk; then cp config.mk.dist sub/config.mk; fi
	@$(MAKE) --no-print-directory -C sub
