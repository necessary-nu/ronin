# `+=` reads through the binding and defines the answer outside it, so the loop
# word is what gets pasted onto and the global is what ends up holding it.
x = FAIL
$(foreach x,FAIL PASS,$(eval x += $(x)))
first := [$(x)] [$(flavor x)] [$(origin x)]
x = PASS
$(foreach x,FAIL,)
second := [$(x)] [$(flavor x)] [$(origin x)]
all: ; @printf '%s\n' '$(first)' '$(second)' > out
