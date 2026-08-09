files := foo.elc bar.o lose.o foo.i bar.i
all: ; @printf '[%s]\n' '$(filter %.o,$(files))' '$(filter %.o foo.i,$(files))' '$(filter nothing,$(files))' > out
