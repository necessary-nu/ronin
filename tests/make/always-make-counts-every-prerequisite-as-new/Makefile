# `$?` is filled by `d->changed || always_make_flag` (commands.c), so under `-B`
# it names every ordinary prerequisite however old it is — and still names no
# order-only one, because those are not in the list at all.
out: old new | ordered
	@printf 'q=[%s] all=[%s]\n' '$?' '$^' > out
ordered: ; @printf 'o\n' > ordered
