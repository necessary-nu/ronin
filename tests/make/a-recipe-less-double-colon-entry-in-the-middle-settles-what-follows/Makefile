# The rename is per entry and reaches forward only. The first entry has work
# and remakes the target where the Makefile wrote it; the second has no recipe,
# is found current and hands the found path to the third; the third remakes the
# file down the search path. One chain, two files written, in one run.
VPATH = src
out.o:: late
	@echo "one at=$@" >> log.txt
	@echo one >> $@
out.o:: keep
out.o:: late
	@echo "three at=$@" >> log.txt
	@echo three >> $@
