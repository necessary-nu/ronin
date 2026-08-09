# Each double-colon rule is weighed against the target on its own
# prerequisites, so only the rule whose prerequisite is newer runs.
out:: newer.in ; @printf '%s\n' from-newer >> log
out:: older.in ; @printf '%s\n' from-older >> log
