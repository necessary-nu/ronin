define NEWLINE


endef
d = outer
$(foreach d,a b,$(eval define d$(NEWLINE)deep$(NEWLINE)endef))
all: ; @printf '%s\n' 'd=[$(d)] $(flavor d) $(origin d)' > out
