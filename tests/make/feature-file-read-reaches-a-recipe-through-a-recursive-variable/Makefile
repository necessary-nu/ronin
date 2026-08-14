# The Linux kernel's `read-file`, reduced. `scripts/Kbuild.include` defines it
# as `$(subst $(newline),$(space),$(file < $1))`, a recursively expanded
# `KERNELRELEASE` calls it, and a recipe interpolates `KERNELRELEASE` — so
# expanding the recipe is what reads the file. There is nowhere to defer the
# read to: its result has to reach the `subst` around it.
define newline


endef
empty :=
space := $(empty) $(empty)
read-file = $(subst $(newline),$(space),$(file < $1))
KERNELRELEASE = $(call read-file, kernel.release)
CONFIGURED = $(call read-file, two-lines)

all: utsrelease.h

utsrelease.h: kernel.release two-lines
	@printf 'UTS_RELEASE "%s"\n' '$(KERNELRELEASE)' > $@
	@printf 'CONFIGURED [%s]\n' '$(CONFIGURED)' >> $@
