# The same rule with the mentioned name written FIRST. Which name the walk
# reaches first is not something the makefile said, so it cannot decide what is
# swept: this leaves exactly what the case beside it leaves.
a.4:
%.4: a.3 %.1 %.15 ; cat $^ >$@
%.1 %.15 %.3: ; touch $*.1 $*.15 $*.3
