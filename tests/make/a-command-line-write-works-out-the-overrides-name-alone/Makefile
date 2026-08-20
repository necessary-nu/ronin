# A command-line write fills `command_variables`, which is what defines
# MAKEOVERRIDES and the hidden table it refers to. It is not a goal word, so it
# never reaches the branch that defines MAKECMDGOALS: an invocation can override
# a name without ever naming a goal.
ifeq ($(origin MAKECMDGOALS),undefined)
all: no-goal-was-named
else
all: a-goal-was-named
endif

no-goal-was-named: ; @printf 'branch=[none] over=[%s] value=[%s] hidden=[%s] table=[%s] foo=[%s]\n' '$(origin MAKEOVERRIDES)' '$(MAKEOVERRIDES)' '$(origin -*-command-variables-*-)' '$(-*-command-variables-*-)' '$(FOO)' > out
a-goal-was-named: ; @printf 'branch=[some]\n' > out
