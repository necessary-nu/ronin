# The other half of the same rule: when no Makefile's own update succeeded there
# is nothing to raise `any_remade`, so both failures are forgiven and the read
# stands as it was.
all: ; @printf 'restarts=%s one=%s two=%s\n' '$(MAKE_RESTARTS)' '$(ONE)' '$(TWO)' > out

-include one.mk
-include two.mk

one.mk: ; @printf 'one ran\n' >> log; exit 1
two.mk: ; @printf 'two ran\n' >> log; exit 1
