# -q over a target -W has already answered about. There is nothing to do, so the
# answer is 0 rather than 1 — the only case in this family where the switch
# changes an exit status rather than what reaches the disk.
pre.out:
	@echo before > pre.out
	@$(MAKE) -f sub.mk sub
	@echo after >> pre.out
