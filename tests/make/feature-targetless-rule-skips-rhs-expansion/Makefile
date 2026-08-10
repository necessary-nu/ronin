# GNU Make accepts targetless rules without examining their prerequisites, and
# ignores recipe lines belonging to them. Cover every rule-separator spelling
# both literally and when the separator arrives by expansion.
SIDE = $(error TARGETLESS_RHS_EXPANDED)
COLON = :
DOUBLE_COLON = ::
GROUPED_COLON = &:

: $(SIDE)
	@false
:: $(SIDE)
	@false
&: $(SIDE)
	@false
$(COLON) $(SIDE)
	@false
$(DOUBLE_COLON) $(SIDE)
	@false
$(GROUPED_COLON) $(SIDE)
	@false

.PHONY: all
all: ; @printf '%s\n' ok > all-out
