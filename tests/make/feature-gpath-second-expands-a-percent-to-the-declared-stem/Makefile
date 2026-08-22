# A `%` in a static pattern rule's second-expanded prerequisites stands for the
# stem the read worked out, and the read worked it out against the name as
# written. So the rename does not send it looking for build/out.h.
.SECONDEXPANSION:
VPATH = build
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: %.o: %.c $$(addsuffix .h,%) ; @printf 'made [%s] whole [%s] stem [%s]\n' '$@' '$^' '$*' > $@
