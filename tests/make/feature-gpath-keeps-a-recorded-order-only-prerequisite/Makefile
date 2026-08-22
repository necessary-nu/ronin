# The order-only half of the record travels with the rest of it: `%.h` was
# filled in for the name as written when the rule was read, and the rename does
# not send it looking for build/out.h.
VPATH = build
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: %.o: %.c | %.h ; @printf 'made [%s] from [%s] bar [%s]\n' '$@' '$<' '$|' > $@
