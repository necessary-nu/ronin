# Make defines MAKEFLAGS at the rank `-e` gives the environment, so a makefile
# writing to it is outranked and the flag never arrives — the built-in rules
# `-r` would have withdrawn are still there.
MAKEFLAGS += -r
all: ; @printf '%s\n' '$(origin MAKEFLAGS)' '$(MAKEFLAGS)' '$(origin CC)' > out
