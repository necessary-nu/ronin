# The quiet-command hoist asks whether a recipe silenced every one of its
# lines, and only a per-line reading of the prefixes can answer. This macro
# silences the echo and leaves the work loud, so GNU Make prints the quiet
# line and then echoes `cp loud.c loud.o` before running it. A reading that
# took the first expanded line's `@` for the whole expansion would call the
# recipe wholly silenced, hoist the echo into the description, and lose the
# line GNU Make echoes.
define quiet
@echo '  CC      $@'
cp $< $@
endef

all: loud.o

loud.o: loud.c ; $(quiet)
