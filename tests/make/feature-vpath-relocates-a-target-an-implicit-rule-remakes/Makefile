# The same throwing away, for a target no explicit rule names: the search finds
# an older copy, the implicit rule search matches the name as written, and what
# it makes lands here.
VPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

%.o: %.c ; @printf '%s from %s\n' '$@' '$<' > $@
