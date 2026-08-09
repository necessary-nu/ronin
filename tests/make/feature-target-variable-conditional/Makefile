# ?= in a target-specific assignment sets the name only where it has no value
# already.
all: FOO ?= from-target

all: ; @printf '%s\n' '$(FOO)' > out
