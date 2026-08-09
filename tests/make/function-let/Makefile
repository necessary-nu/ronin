a = bad
b = news
x = $(let a b,1 2,$(a) $(b))
y = $(let a,1 2,$(a))
z = $(let a b,1,$(a) $(b))
all: ; @printf '[%s]\n' '$(a)' '$(b)' '$(x)' '$(y)' '$(z)' > out
