dirs := a b c
all: ; @printf '[%s]\n' '$(foreach d,$(dirs),$(d)/lib.a)' '$(foreach d,,$(d)/lib.a)' > out
