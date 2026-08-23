# What the missing expansion reaches, as an effect rather than as a word: a
# `.SHELLFLAGS` value holding `$(...)` is Make's to expand, and GNU Make expands
# it into nothing — 4.4.1 reads an unknown function name as a variable reference
# and answers empty. A build that copies the value through instead hands the
# parentheses to whatever reads the assembled command line, where they are not
# Make's any more: rkati's launch goes through `/bin/sh`, which runs what is
# inside them and leaves `pwned` behind, and this front end's own shell splits
# the line at the blank and dies of the half-substitution it is left holding.
#
# `pwned` is what the substitution leaves behind and GNU Make 4.4.1 never leaves
# it; the recording gates that with the file it does leave and the build that
# succeeded to leave it.
SHELL := /bin/sh
.SHELLFLAGS := -c $$(touch pwned)
all:
	@echo hello > out
