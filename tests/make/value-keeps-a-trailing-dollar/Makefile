S := x$
R = y$
define D
z$
endef
E := $(subst a,b,q$)
F := a$$
all:
	@printf '[%s]\n' '$(S)' '$(value S)' '$(R)' '$(value R)' '$(D)' '$(value D)' '$(E)' '$(F)' '$(words $(S))' > out
	@exec >>out; printf 'recipe=%s\n' A$
