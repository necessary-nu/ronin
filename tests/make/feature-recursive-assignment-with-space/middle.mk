ifneq ($(V),two words)
$(error recursive assignment was '$(V)')
endif
middle:
	@printf '%s\n' '$(V)' > middle
	$(MAKE) --no-print-directory -f bottom.mk bottom
.PHONY: middle
