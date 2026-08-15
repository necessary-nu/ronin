# A shuffle reorders which prerequisite is considered first and nothing else.
# GNU Make populates a second `->shuf` chain over the same prerequisites and
# leaves `->next` alone, and `->next` is what the automatic variables walk — so
# $^, $+ and $< read the order the Makefile wrote however the build was
# reordered, including on the target whose own edge the drop took away.
all: t
t: p q r ; @printf 'hat=[$^] first=[$<] plus=[$+]\n' > t.out
p: ; @printf '$@\n' >> order
q: t ; @printf '$@\n' >> order
r: ; @printf '$@\n' >> order
