# The built-in catalogue is not what a bare `export` means: GNU Make leaves an
# `o_default` variable out of every environment unless a directive names it.
export
all: ; @printf 'CC=[%s] CXX=[%s] MINE=[%s]\n' "$$CC" "$$CXX" "$$MINE" > out
MINE = mine
