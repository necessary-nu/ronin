# The names an `export` directive takes are expanded, and the variables they
# name need not exist yet: the attribute is set now and the value read when the
# child starts.
NAMES = A B
export $(NAMES)
A = a
B = b
EMPTY =
# An expansion that produces no words names no variable — and is not the bare
# directive either, which GNU Make decides on the unexpanded text.
export $(EMPTY)
C = c
all: ; @printf 'A=%s B=%s C=[%s]\n' "$$A" "$$B" "$$C" > out
