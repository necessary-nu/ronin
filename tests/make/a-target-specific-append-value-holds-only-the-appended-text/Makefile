# GNU Make's $(value) of a target-specific `+=` reads the appended text alone
# when the base it builds on lives in a parent set: variable_append stores only
# that text on the binding and walks the parent for the base at expansion. The
# base is stored on the binding — and so read back by $(value) — only when an
# earlier assignment or append in the SAME set put it there.
#
# V's base `g` is global, so $(value V) is the two appends alone; W's base is a
# `:=` in a.o's own set, so $(value W) carries it. The expansion of each carries
# the base either way, because $(V) walks to it whether it was stored or found.
V = g

a.o: V += <one>
a.o: V += <two>
a.o: W := inset
a.o: W += <tail>

all: a.o
a.o: ; $(file >v-value.out,[$(value V)])$(file >w-value.out,[$(value W)])$(file >expanded.out,[$(V)] [$(W)])
