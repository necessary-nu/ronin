n = -10
z = 0
p = 888
all: ; @printf '[%s]\n' '$(intcmp $(n),$(n))' '$(intcmp $(n),$(z))' '$(intcmp $(n),$(z),lt)' '$(intcmp $(n),$(p),lt,ge)' '$(intcmp $(z),$(z),lt,ge)' '$(intcmp $(z),$(p),lt,eq,gt)' '$(intcmp $(p),$(z),lt,eq,gt)' '$(intcmp $(p),$(p),lt,eq,gt)' > out
