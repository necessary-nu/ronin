# GNU Make expands a recipe when it is about to run it and not before, so a
# `$(file >)` inside the recipe of a target that is already up to date never
# happens at all.
#
# A grouped `&::` record is one recipe for the whole group, and its members
# here are current, so GNU Make runs nothing and writes no file. Ronin holds
# such a recipe unexpanded and expands it at launch, which is where GNU Make
# performs the write too — never, for a group that is up to date.
all: a b

a b &:: dep
	$(file > written,made)
	@printf 'ran\n' >> log
