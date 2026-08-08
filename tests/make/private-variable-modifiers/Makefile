override private A = a
private override B = b
GLOBAL := A [$(A)] $(origin A) B [$(B)] $(origin B)
all: export override private C = c
all: ; @printf '%s\n' '$(GLOBAL)' 'recipe A [$(A)] B [$(B)] C [$(C)]' > out
