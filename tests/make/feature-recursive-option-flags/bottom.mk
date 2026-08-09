ifeq ($(findstring -j2,$(MAKEFLAGS)),)
$(error child MAKEFLAGS lost -j2)
endif
ifneq ($(findstring -j3,$(MAKEFLAGS)),)
$(error child MAKEFLAGS retained -j3)
endif
ifeq ($(findstring -l4,$(MFLAGS)),)
$(error child MFLAGS lost -l4)
endif
bottom: ; @printf '%s\n' replaced > replaced
.PHONY: bottom
