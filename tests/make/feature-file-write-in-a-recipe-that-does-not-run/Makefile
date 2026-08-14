# GNU Make expands a recipe when it is about to run it and not before, so a
# `$(file >)` inside the recipe of a target that is already up to date never
# happens at all.
#
# The `$?` is what makes this a case of its own rather than a repeat. Ronin
# holds a recipe unexpanded and expands it at launch, which is what puts a file
# operation where GNU Make puts it — but a recipe naming `$?` is one the
# compiler has to read before the graph is complete, so that one is expanded
# while the graph is built, and the write goes with it.
all: target

target: dep
	$(file > written,$?)
	@printf 'ran\n' >> log
