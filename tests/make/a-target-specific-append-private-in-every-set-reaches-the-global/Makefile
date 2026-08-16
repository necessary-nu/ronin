# `private' on the append itself takes the append out of a prerequisite's walk
# altogether rather than just changing its base, and the walk carries on to
# whatever set is next. With both sets private the prerequisite is left with
# the global value, unappended; with only the target's own private it is left
# with the pattern set's append. Inheritance is a chain, so a prerequisite's
# own prerequisite reads the same answer.
V = g
%.o: private V += p
a.o: private V += t

W = g
%.o: W += p
a.o: private W += t

all: a.o
a.o: dep ; @echo "own V=[$(V)] W=[$(W)]" >> log
dep: kid ; @echo "dep V=[$(V)] W=[$(W)]" >> log
kid: ; @echo "kid V=[$(V)] W=[$(W)]" >> log
