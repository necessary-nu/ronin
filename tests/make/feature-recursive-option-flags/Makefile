# Job and load flags stay visible while recursive Makefiles compile into one
# graph; a child spelling replaces what its parent exposed without adding a
# second scheduler.
ifeq ($(findstring -j3,$(MAKEFLAGS)),)
$(error root MAKEFLAGS lost -j3)
endif
ifeq ($(findstring -l2.5,$(MAKEFLAGS)),)
$(error root MAKEFLAGS lost -l2.5)
endif
all:
	$(MAKE) --no-print-directory -f middle.mk middle
.PHONY: all
