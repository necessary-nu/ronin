ifneq ($(V),two words)
$(error inherited assignment was '$(V)')
endif
bottom: ; @printf '%s\n' '$(V)' > result
.PHONY: bottom
