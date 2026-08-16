# One recipe standing for several targets still reads `$*' off the single name
# `$@' was bound to: a grouped rule's stem is the member that was reached, and
# a `::' record's is its own target. The suffix walk is the same one in both.
all: a.c b.c c.c
a.c:: ; @echo "double-colon=[$*]" >> log
b.c c.c &: ; @echo "grouped=[$*] at=[$@]" >> log
