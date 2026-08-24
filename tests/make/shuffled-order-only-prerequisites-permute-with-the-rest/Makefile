# GNU Make carries a target's normal and order-only prerequisites in ONE `->next`
# chain, telling the two apart per entry with `ignore_mtime`, and shuffles that
# one chain — so under `--shuffle` an order-only prerequisite permutes among the
# normal ones. Ronin keeps two lists (`deps`, `order_onlys`) and shuffles each
# within itself, so the two groups never cross. `$^` and `$|` agree either way;
# what differs is the order the four independent targets are built in.
all: t
t: a b | c d
	@printf '%s\n' 'hat=$^' > hat
	@printf '%s\n' 'pipe=$|' >> hat
a: ; @printf 'a\n' >> order
b: ; @printf 'b\n' >> order
c: ; @printf 'c\n' >> order
d: ; @printf 'd\n' >> order
