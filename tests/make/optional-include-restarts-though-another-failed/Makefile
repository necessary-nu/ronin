# GNU Make decides about each Makefile it read and not about the batch: it
# raises `any_remade` from the ones whose own update succeeded (main.c). One
# `-include` that was remade therefore starts the read over even though the
# rule for another lost in the same pass, and the loser is still forgiven.
all: ; @printf 'restarts=%s one=%s two=%s\n' '$(MAKE_RESTARTS)' '$(ONE)' '$(TWO)' > out

-include one.mk
-include two.mk

one.mk: ; @printf 'ONE := yes\n' > $@
two.mk: ; @printf 'two ran\n' >> log; exit 1
