# A leg that is not taken is skipped whole, assignments included, and the rule
# that follows the conditional still belongs to whichever leg was read.
CHOSEN = default

ifneq ($(PICK),yes)
all: ; @printf 'chosen=%s\n' '$(CHOSEN)' > out
else
CHOSEN = picked
all: ; @printf 'chosen=%s\n' '$(CHOSEN)' > out
endif
