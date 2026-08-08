private F = g
GLOBAL := [$(F)] $(origin F) $(flavor F)
all: ; @printf '%s\n' 'global $(GLOBAL)' \
	'recipe [$(F)] $(origin F) $(flavor F)' > out
