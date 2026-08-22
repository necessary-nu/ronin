# -o has toenv clear in the switch table (main.c:484), so it is not written
# into MAKEFLAGS and a recursive child is never told. The child remakes sub
# exactly as it would have without the switch.
pre.out:
	@echo before > pre.out
	@$(MAKE) -f sub.mk sub
	@echo after >> pre.out
