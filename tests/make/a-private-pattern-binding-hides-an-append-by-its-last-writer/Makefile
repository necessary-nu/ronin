# Every matching pattern lands in one variable set, which is one hash table
# slot per name, so the set is left holding one `private' flag: the last
# pattern's. What a prerequisite's append finds for its base is decided by that
# one flag over the whole accumulated set, not by the pattern that wrote the
# part of it being read.
#
# V: private first, public second -- the whole set shows through.
# W: public first, private second -- the whole set is stepped over.
V = g
%.o: private V += p1
a.%: V += p2
a.o: V += t

W = g
%.o: W += p1
a.%: private W += p2
a.o: W += t

all: a.o
a.o: dep ; @echo "own V=[$(V)] W=[$(W)]" >> log
dep: ; @echo "dep V=[$(V)] W=[$(W)]" >> log
