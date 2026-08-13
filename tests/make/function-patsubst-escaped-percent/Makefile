V := abc def
X := a%b a\%b
all: ; @printf '[%s]\n' '$(patsubst \%x,Y,%x ax bx)' '$(patsubst %x,\%,ax bx)' \
 '$(patsubst \%%,Y,%ax %bx cx)' '$(patsubst abc,\%,abc def)' '$(patsubst abc,x\%y,abc)' \
 '$(V:abc=\%)' '$(V:%c=\%)' '$(X:\%=Q)' > out
