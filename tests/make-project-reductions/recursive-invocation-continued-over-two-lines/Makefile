# The kernel's top-level Makefile re-invokes itself once to set
# --no-print-directory, and writes that invocation over two lines:
#
#   __sub-make:
#           $(Q)$(MAKE) $(no-print-directory) -C $(abs_objtree) \
#           -f $(abs_srctree)/Makefile $(MAKECMDGOALS)
#
# A backslash before a newline is the escape that stands for nothing: the
# shell removes the pair and joins what is on either side of it. Reading it
# as an escaped newline put that byte at the front of the following word, so
# the child was asked to build a goal spelled with a newline in front of the
# `-f`, and no external kernel module could be built at all.
child = child.mk

joined:
	@$(MAKE) --no-print-directory -C . \
	-f $(child) made

quoted:
	@$(MAKE) -f "chi\
ld.mk" also-made

both: joined quoted
.PHONY: both joined quoted
