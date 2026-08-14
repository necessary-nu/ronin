# A replaced rule is reinstalled at the end of the list, so a rule written
# after the one it replaces is tried before it from then on. Both stems here
# are the same length, which leaves that order as the whole answer.
%.gen: %.src ; @printf '%s\n' fromsrc > $@
%.gen: %.alt ; @printf '%s\n' fromalt > $@
%.gen: %.src ; @printf '%s\n' fromsrc-again > $@

all: foo.gen
