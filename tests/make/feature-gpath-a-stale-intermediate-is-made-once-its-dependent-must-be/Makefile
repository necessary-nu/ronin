# The other half of the same rule: app is older than out.c, so app must be made,
# and `update_file_1`'s second loop then goes over its intermediate
# prerequisites and updates each one that is out of date on its own account.
VPATH = build
GPATH = build

.SECONDARY: out.o

all: app

app: out.o ; @cat $< > $@

out.o: out.c ; @printf 'made\n' > $@
