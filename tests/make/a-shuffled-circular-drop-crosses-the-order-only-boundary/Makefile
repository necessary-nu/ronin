# The one cell where the two-list shuffle reaches a value a makefile can read.
# GNU Make's circular drop unlinks the entry the written chain stands on while
# the shuffled walk is somewhere else (remake.c:596), and its one chain holds
# the order-only prerequisites too — so the position the drop takes can be on
# the other side of the `|`, and `$|` loses `b` rather than the `t` that closed
# the loop. Ronin's two lists put a boundary there that GNU does not have, so
# its drop stays inside the order-only list.
all: t
t: a | b t c
	@printf '%s\n' 'hat=$^' > hat
	@printf '%s\n' 'pipe=$|' >> hat
a: ; @printf 'a\n' >> order
b: ; @printf 'b\n' >> order
c: ; @printf 'c\n' >> order
