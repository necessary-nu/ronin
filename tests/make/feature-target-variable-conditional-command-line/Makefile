# The same conditional assignment, with the name given on the command line: the
# command line already gave it a value, so the target's is not used.
all: FOO ?= from-target

all: ; @printf '%s\n' '$(FOO)' > out
