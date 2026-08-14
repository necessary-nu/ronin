# A positional parameter is bound the same way a loop variable is.
1 := foo
define param
$(eval 1:=bar)param1-1=$(1) $(call param2,$(1))
endef
define param2
param2-1=$(1)
endef
all: ; @printf '%s\n' 'call [$(call param,baz)]' '1=[$(1)]' > out
