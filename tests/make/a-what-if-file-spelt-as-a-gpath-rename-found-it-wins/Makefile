# The found spelling, where `-W` and `-o` part company. Both make a file object
# of their own for a name that is not a target when the switch is read, and the
# rename merges it with the one the search moved — keeping the later of the two
# dates, under a comment in file.c that says so outright: `%%% Kludge so -W wins
# on a file that gets vpathized`. An asserted NEW date is later than any real
# one, so it wins and app is remade.
VPATH = src
GPATH = src
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
