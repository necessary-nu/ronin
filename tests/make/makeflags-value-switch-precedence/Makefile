# The switches that carry something rather than only being given: what reaches
# a sub-make is a value, and one it settles on itself replaces that value.
all:
	@OUT=inherited $(MAKE) -f sub.mk
	@OUT=other-sync $(MAKE) -Otarget -f sub.mk
	@OUT=more-debug $(MAKE) --debug=j -f sub.mk
	@OUT=no-debug $(MAKE) --debug=n -f sub.mk
	@OUT=no-shuffle $(MAKE) --shuffle=none -f sub.mk
