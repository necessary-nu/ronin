# The mirror: what reaches each sub-make is a withdrawal, and the command line
# asks for the switch outright. A withdrawal nobody argues with still stands.
all:
	@OUT=inherited $(MAKE) -f sub.mk
	@OUT=asserted-w $(MAKE) -w -f sub.mk
	@OUT=asserted-s $(MAKE) -s -f sub.mk
	@OUT=asserted-k $(MAKE) -k -f sub.mk
