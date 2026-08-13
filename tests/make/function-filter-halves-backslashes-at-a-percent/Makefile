W := foo%bar fooXbar foo\%bar foo\Xbar
all: ; @printf '[%s]\n' '$(filter foo\\%bar,$(W))' '$(filter foo\\\%bar,$(W))' \
 '$(filter-out foo\%bar,$(W))' '$(filter %x\%y,ax\%y aXy ax%y)' '$(filter \%%,%a %b c)' \
 '$(filter a%b%c,a%b%c axb%c ab%c)' > out
