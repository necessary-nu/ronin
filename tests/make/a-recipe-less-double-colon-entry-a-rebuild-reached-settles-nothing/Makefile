# What the recipe-less entry IS decided by: whether a prerequisite was
# actually remade. `deps_changed` gathers a bit set where the prerequisite was
# rebuilt, and a set bit withholds the overruling — so the entry is out of date
# and renames nothing, and the entry after it writes the name as written. The
# prerequisite here is absent and has a rule, so this run makes it.
VPATH = src
out.o:: keep
out.o:: late
	@echo "two at=$@" >> log.txt
	@echo two >> $@
keep:
	@echo "made keep" >> log.txt
	@touch keep
