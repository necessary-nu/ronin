VARS = A B
VARSET = $(1) = $(2)
$(foreach v,$(VARS),$(eval $(call VARSET,$(v),$(v))))
all: ; @printf '%s\n' 'A = $(A) B = $(B)' > out
