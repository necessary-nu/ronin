# A generated Makefile is made before the makefile means what it will mean, and
# making it does not make it the goal: a bare make still builds the first
# explicit rule, and the rule that generates the include is not it.
-include gen.mk

first: ; @printf '%s\n' '$(FROM_GENERATED)' > out

gen.mk: ; @printf 'FROM_GENERATED := generated\n' > $@
