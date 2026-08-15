# And the mirror of it: one included before the missing one is reached first,
# so its rule runs and its work stands, and only then does the run end.
include gen.mk
include nope.mk

all: ; @printf 'all ran\n' > out

gen.mk: gen.src ; @printf 'gen\n' >> log; touch gen.mk
gen.src: ; @printf 'src\n' >> log; touch gen.src
