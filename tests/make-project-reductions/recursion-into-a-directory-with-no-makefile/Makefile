# The shape a relic tree keeps, reduced from lang-sme's
# devtools/testing-from-old-infra/make-dictindex: a recipe line whose own
# command IS the invocation -- so the compiler composes it -- pointed at a
# directory that holds no makefile at all. There the line is
# `@cd $(SRCDIR)/ && $(MAKE) ifst GTLANG=sme' and `SRCDIR' is two levels up, at
# a directory holding only a Makefile.am.
SRCDIR = nomakefile

all: before missing after

before:
	cd present && $(MAKE) all

missing:
	@cd $(SRCDIR)/ && $(MAKE) all

after:
	@if test -n "$(WHEN)"; then cd present && $(MAKE) all; fi

# The other thing a relic tree is full of, and a finding that is not a census
# entry: a target declared twice, whose later recipe wins.
noise:
	@echo the first recipe

noise:
	@echo the later recipe wins
