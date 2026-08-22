.POSIX:

# A recipe over the length that decides a script cannot be an argument,
# generated here so the case stays a case rather than a hundred kilobytes of
# checked-in padding. `x` ten times, then four doublings by ten, then eleven of
# those: 110,000 bytes of `:` argument in one recipe line.
P1 := xxxxxxxxxx
P2 := $(P1)$(P1)$(P1)$(P1)$(P1)$(P1)$(P1)$(P1)$(P1)$(P1)
P3 := $(P2)$(P2)$(P2)$(P2)$(P2)$(P2)$(P2)$(P2)$(P2)$(P2)
P4 := $(P3)$(P3)$(P3)$(P3)$(P3)$(P3)$(P3)$(P3)$(P3)$(P3)
PAD := $(P4)$(P4)$(P4)$(P4)$(P4)$(P4)$(P4)$(P4)$(P4)$(P4)$(P4)

all: out

# One line, so nothing assembles a `set -e` of its own around it: under
# `.POSIX:` the strictness is the shell's `-ec` and nothing else. The line
# fails in the middle, so `-e` is the whole difference between m1 existing and
# not.
out:
	@: $(PAD) ; false ; touch m1
