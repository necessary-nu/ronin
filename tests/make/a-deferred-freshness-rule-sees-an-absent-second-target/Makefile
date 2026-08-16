# A recipe naming `$?' cannot be expanded when the edge launches, so its
# freshness is asked separately from the scan's own stat loop -- and it has to
# be asked of every name the rule was reached for, not only the one whose
# recipe is about to be expanded. A pattern rule spelling two target patterns
# is one recipe over targets that each answer for themselves.
#
# `a.c' is current and `a.h' is absent; `b.c' is current and `b.h' is behind
# `b.in'. Either is a reason to run, and neither is visible from the first name
# alone.
all: a.c a.h b.c b.h
%.c %.h: %.in ; echo "$* q=$?" >> log ; cat $< > $*.c ; cat $< > $*.h
