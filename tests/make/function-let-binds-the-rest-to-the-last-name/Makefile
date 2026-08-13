x = $(let a b,  1   2   3  ,+$(a)+$(b)+)
y = $(let a b c,1,+$(a)+$(b)+$(c)+)
z = $(let a,,+$(a)+)
w = $(let ,1 2,body)
all: ; @printf '[%s]\n' '$(x)' '$(y)' '$(z)' '$(w)' > out
