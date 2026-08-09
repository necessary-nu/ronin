# An included file that can be built is built, and make then starts again so
# that what it defines is in force.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk: ; @printf 'GENERATED := from-generated\n' > $@
