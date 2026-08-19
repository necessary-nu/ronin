# The archive spelling of the same rule, and the reason the defect was found
# here: `(%): % ;` is how a Makefile says "file the member, the archive rule
# will do the work", and with an empty recipe it does no filing at all. GNU
# Make still makes `a.o` for it — the member is not there, so the rule that
# makes it must run — and then sweeps `a.o` away as the intermediate the
# implicit search invented. No archive is ever written: the recipe that would
# write one is empty.
all: mylib.a(a.o)

(%): % ;

%.o : %.c
	@echo Compile $< >> log; touch $@
