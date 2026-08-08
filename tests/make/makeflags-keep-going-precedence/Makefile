# Keep going reaches both sub-makes through MAKEFLAGS. The first takes it back
# on its own command line and stops at the failure; the second says it again
# and carries on to the target beside it.
all:
	-@$(MAKE) -S -f stop.mk
	-@$(MAKE) -k -f go.mk
