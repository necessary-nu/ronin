a = outer
b = news
inner = $(let a,shadow,$(a)/$(origin a)/$(flavor a))
nested = $(let a b,1 2,$(let a,9,$(a)$(b)))
all: ; @printf '[%s]\n' '$(inner)' '$(a)' '$(nested)' '$(a)$(b)' > out
