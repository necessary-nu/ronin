# `a.1` is named by a rule that has nothing to do with making `a.3`, so it is
# mentioned and stays. `a.2` comes off the same action, was invented by the
# search, and goes. One action, two answers.
a.3:
%.3: %.1 %.2 ; cat $^ >$@
%.1 %.2: ; touch $*.1 $*.2
install: a.1
.PHONY: install
