files := foo.elc bar.o lose.o foo.i bar.i
all: ; @printf '[%s]\n' '$(filter-out %.o,$(files))' '$(filter-out foo.i bar.i %.o,$(files))' > out
