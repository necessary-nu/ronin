# `--always-make` withholds the overruling — the clause reads
# `&& !always_make_flag` — so a recipe-less entry is decided by the ordinary
# timestamp comparison again. It reaches the entry through `file->cmds != 0`
# and so never forces one with no recipe: `keep` is older than the target here,
# the comparison finds nothing wrong, and the chain is renamed under a switch
# whose whole point is to remake everything.
VPATH = src
out.o:: keep
out.o:: late
	@echo "two at=$@" >> log.txt
	@echo two >> $@
