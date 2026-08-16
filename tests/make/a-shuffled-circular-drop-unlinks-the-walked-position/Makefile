# GNU Make's circular drop names one prerequisite and unlinks another. The walk
# carries two chains over the same list — `->next` in written order and `->shuf`
# in the shuffled one — and reads the file through the shuffled chain while
# unlinking through the written one (remake.c:596), so the entry that goes is
# the one standing where the walk had got to rather than the one that closed the
# loop. Written order makes the two the same entry and nothing shows; under
# --shuffle=reverse the walk reaches `t` while the written chain is still at
# `b`, so `b` is what leaves the list and `$^` is left holding `t`.
all: t
t: a b t c
	@printf '%s\n' '$^' > hat
a: ; @printf 'a\n' >> order
b: ; @printf 'b\n' >> order
c: ; @printf 'c\n' >> order
