# A pattern-specific value is expanded where the makefile is, not inside the
# scope being built: `$(N)` here is the global one, though the same pattern
# sets `N` for every target it matches.
N = global

%.o: N = from-pattern
%.o: X := $(N)

all: a.o
a.o: ; @printf 'X=%s N=%s\n' '$(X)' '$(N)' > $@
