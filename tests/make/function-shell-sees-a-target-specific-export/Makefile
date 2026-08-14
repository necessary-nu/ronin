# `func_shell` builds its child's environment with `target_environment(NULL)`,
# whose NULL means the variable sets that are current where the call is being
# expanded — so a recipe-time `$(shell)` sees that target's exports.
all: ; @printf '[%s]\n' '$(shell echo $$TS)' > out
all: export TS = t
