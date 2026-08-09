DEP_foo = bar baz quux
DEP_baz = quux blarp
rest = $(wordlist 2,$(words $(1)),$(1))
tclose = $(if $(1),$(firstword $(1)) $(call tclose,$(sort $(DEP_$(firstword $(1))) $(call rest,$(1)))))
all: ; @printf '[%s]\n' '$(call tclose,foo)' > out
