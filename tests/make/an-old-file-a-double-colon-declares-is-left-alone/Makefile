# enter_file returns the entry it found only when that entry is not a
# double-colon target (file.c), so a switch that stamps a name by entering it
# stamps a fresh file object no scan consults. -o over out does exactly what no
# switch at all does: out is newer than a and b, so neither recipe runs, and app
# is older than out, so app is made.
all: app
app: out
	@echo made > $@
out:: a
	@echo one >> $@
out:: b
	@echo two >> $@
