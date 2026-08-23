# The rule the recipe-less entry is found current BY, which is not the rule
# every other entry is found current by. `remake.c` overrules the timestamp
# comparison outright for a target that exists and has no recipe, so a
# prerequisite NEWER than the target decides nothing at all: the entry is still
# not remade, and it still renames the chain. `keep` is newer than the target
# here and the same rename happens.
VPATH = src
out.o:: keep
out.o:: late
	@echo "two at=$@" >> log.txt
	@echo two >> $@
