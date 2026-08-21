# -B outranks -W: always_make_flag makes every target with a recipe out of date
# whatever a date says, and -W only writes a date. The boundary of the fix, and
# it passed before it as well as after.
pre.out:
	@echo before > pre.out
	@$(MAKE) -f sub.mk sub
	@echo after >> pre.out
