# The two readings of an archive member meeting in one list. GNU Make answers
# `$?` with `b.o` alone, and both halves of that need a rule the other one does
# not.
#
# `mylib.a(b.o)` is not in the archive at all. Its rule's recipe is empty, so
# nothing ever files it — and a target a recipe was run for and did not write is
# one that does not exist, which GNU Make counts as newer than whatever reads
# it. Read as an alias for `b.o` instead it would take `b.o`'s date and drop out
# of the list.
#
# `mylib.a(a.o)` is in the archive, dated the whole second `a.o` was written .7
# of the way through. Read as the target of its own rule it is the end of that
# second and current; read as `mylib.a`'s prerequisite it is the start of it and
# older than the archive, so it is not new and does not belong in `$?`.
mylib.a: mylib.a(a.o) mylib.a(b.o)
	@echo "Q=[$?]" >> log

(%): % ;
