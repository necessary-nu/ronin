# The Linux kernel's `syncconfig` reduced: a multi-target PATTERN rule whose
# recipe is a RECURSION, making files an INCLUDE needs. The kernel writes it as
# `%/config/auto.conf %/config/auto.conf.cmd ...: $(KCONFIG_CONFIG)` carrying
# `$(MAKE) -f $(srctree)/Makefile syncconfig`, under its own comment "This
# exploits the 'multi-target pattern rule' trick."
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen/auto.conf

%/auto.conf %/auto.conf.cmd: base.config
	@printf 'SYNC %s\n' '$@' >> sync.log
	@$(MAKE) -f sync.mk sync
