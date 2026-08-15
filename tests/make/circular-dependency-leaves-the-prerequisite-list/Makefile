# The dropped edge is unlinked from the list it was on, and that list is what
# the automatic variables are read from — so the target whose prerequisite was
# dropped never sees it again, while the target it pointed at keeps all of its
# own.
all: a
a: b c ; @printf 'a hat=[$^] q=[$?] plus=[$+]\n' > a.out
b: a ; @printf 'b hat=[$^] plus=[$+]\n' > b.out
c: ; @printf 'c\n' > c.out
