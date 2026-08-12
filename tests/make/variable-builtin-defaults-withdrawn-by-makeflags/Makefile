# The Linux kernel's build asks for a clean namespace this way, and the switch
# arrives while the Makefile is still being read. The catalogue stays in scope
# for the rest of the read and is gone by the time a recipe expands.
MAKEFLAGS += -rR

DURING := $(origin CC)/$(COMPILE.c)

all: ; printf '[%s]\n' '$(DURING)' '$(COMPILE.c)' '$(OUTPUT_OPTION)' '$(origin CC)' > out
