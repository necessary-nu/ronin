A := $(shell printf 'a\r\nb\rc\n\r\n')
B != printf 'a\r\nb\r\n\r\n'
C != printf '\n\n\n'
D != printf 'x'
all: ; @printf '%s\n' 'A=[$(A)]' 'B=[$(B)]' 'C=[$(C)]' 'D=[$(D)]' | cat -A > out
