# Every switch here reaches each sub-make through MAKEFLAGS, and each of them
# says something else about one of them on its own command line. What the
# sub-make settled on is what it would hand a make of its own.
all:
	@OUT=inherited $(MAKE) -f sub.mk
	@OUT=withdrawn-w $(MAKE) --no-print-directory -f sub.mk
	@OUT=withdrawn-s $(MAKE) --no-silent -f sub.mk
	@OUT=withdrawn-k $(MAKE) -S -f sub.mk
	@OUT=agreeing $(MAKE) -k -f sub.mk
	@OUT=added $(MAKE) -r -f sub.mk
