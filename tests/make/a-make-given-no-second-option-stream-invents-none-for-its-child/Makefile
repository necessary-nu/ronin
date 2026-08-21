# The other half of the descent, and the reason GNU Make guards its first
# decode with `if (lookup_variable (… GNUMAKEFLAGS_NAME …))`: emptying a name
# and withdrawing it are two different things, and a Make that was given no
# second option stream must not put one in its child's environment.
#
# Upstream asks this with `env | grep GNUMAKEFLAGS`; asked here through the
# origin instead, which says the same thing without a pipeline: `default` is
# the catalogue's own binding, reached only because nothing was inherited.
all:
	@$(MAKE) --no-print-directory -f child.mk
