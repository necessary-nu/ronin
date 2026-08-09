ifneq ($(origin V),environment)
$(error V descended with origin $(origin V))
endif
ifneq ($(V),from-cli)
$(error V descended with value '$(V)')
endif
child: ; @printf '%s\n' '$(origin V)|$(V)' > result
.PHONY: child
