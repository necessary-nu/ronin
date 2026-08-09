ifeq ($(findstring -j3,$(MAKEFLAGS)),)
$(error inherited MAKEFLAGS lost -j3)
endif
ifeq ($(findstring -l2.5,$(MFLAGS)),)
$(error inherited MFLAGS lost -l2.5)
endif
middle:
	@printf '%s\n' inherited > inherited
	$(MAKE) --no-print-directory -f bottom.mk bottom -j2 -l4
.PHONY: middle
