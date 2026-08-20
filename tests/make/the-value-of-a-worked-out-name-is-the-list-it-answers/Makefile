# `$(value)` reads a variable's stored text without expanding it, and GNU Make's
# `.VARIABLES` is a SIMPLE variable whose text `lookup_special_var` rebuilds as
# it is read — so the text and the expansion are the same bytes by construction
# and `$(value .VARIABLES)` is the list.
#
# The list is never gated verbatim, because it holds the environment. What is
# gated is environment-independent: that a name this makefile defined is in the
# unexpanded answer, that the two answers hold the same number of words, and
# that neither holds a word the other does not.
MY_OWN_NAME := yes

ifeq ($(filter MY_OWN_NAME,$(value .VARIABLES)),MY_OWN_NAME)
all: the-value-is-the-list
else
all: the-value-is-something-else
endif

difference := $(filter-out $(value .VARIABLES),$(.VARIABLES))$(filter-out $(.VARIABLES),$(value .VARIABLES))

the-value-is-the-list: ; @printf 'mine=[%s] words=[%s] difference=[%s]\n' '$(filter MY_OWN_NAME,$(value .VARIABLES))' '$(if $(filter $(words $(value .VARIABLES)),$(words $(.VARIABLES))),same,differ)' '$(difference)' > out
the-value-is-something-else: ; @printf 'value=[%s]\n' '$(value .VARIABLES)' > out
