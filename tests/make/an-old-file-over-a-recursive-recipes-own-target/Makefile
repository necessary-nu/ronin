# Whether a recursive recipe has to run is decided where the graph is composed,
# so the switch has to reach that question too. GNU Make says pre.out is up to
# date and leaves the directory as it found it.
pre.out:
	@echo before > pre.out
	@$(MAKE) -f sub.mk sub
	@echo after >> pre.out
