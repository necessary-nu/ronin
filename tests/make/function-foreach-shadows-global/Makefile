x := GLOBAL
inner := $(foreach x,ONE TWO,[$(x)])
all: ; @printf '[%s]\n' '$(inner)' '$(x)' > out
