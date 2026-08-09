# `include` says the file has to be there. When it is not and no rule can make
# it, the build is refused before any of it happens, even though the goal it
# was asked for has a rule of its own.
include missing.mk

all: ; @printf '%s\n' ok > out
