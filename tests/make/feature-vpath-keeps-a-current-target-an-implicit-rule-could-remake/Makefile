# The implicit-rule half of keeping the found name: the search finds a copy
# that is current, so the rule that could have remade the target never has to,
# and the name its dependents read is the one the search returned.
VPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

%.o: %.c ; @printf '%s from %s\n' '$@' '$<' > $@
