digits := 0 1 2 3 4 5 6 7 8 9
rev = $(eval res :=)$(foreach w,$(1),$(eval res := $(w) $(res)))$(res)
a := $(call rev,$(digits))
all: ; @printf '[%s]\n' '$(a)' > out
