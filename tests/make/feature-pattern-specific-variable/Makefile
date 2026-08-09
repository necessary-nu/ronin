# A variable can be set for every target matching a pattern, and a target's own
# setting, or a longer-matching pattern's, takes precedence.
all: one.x two.x

FOO = foo
BAR = bar

one.x: override FOO = one
%.x: BAR = two
t%.x: BAR = four

one.x two.x: ; @printf '%s %s\n' '$(FOO)' '$(BAR)' > $@
