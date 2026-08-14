# Directives are read in order and the last one to name a variable is the one
# that answers for it, wherever in the file it sits — including after the rule
# whose recipe reads the name.
V = one
export V
unexport V
export V
X = one
export X
all: ; @printf 'V=[%s] X=[%s]\n' "$$V" "$$X" > out
unexport X
