# `enter_file` (file.c) returns the entry it found only when that entry is not a
# double-colon target, so `-W out` gets a FRESH file object appended to the
# chain and the date lands where nothing reads it. Everything that considers
# `out` reads the chain's own entries and their own dates, so the switch is
# inert and `app` — newer than `out` already — is not remade. `out.c` is here so
# the implicit rule search finds the appended entry a rule and the refusal that
# would otherwise fire does not.
all: app
app: out
	@echo made > $@
out:: a
	@echo remade > $@
out:: b
	@echo again >> $@
