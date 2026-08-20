# The other half of GNU Make's one call site. With no SHELL in the environment
# the offer `define_variable_cname ("SHELL", default_shell, o_default, 0)` lands,
# and the name stands `default` and SIMPLE — the trailing zero is the recursive
# flag, and no makefile line can say what that call says. The refusal that
# follows it does not fire, because nothing came from the environment and the
# text it holds is not empty.
ifeq ($(flavor SHELL),simple)
all: the-offer-landed
else
all: something-else-holds-it
endif

the-offer-landed: ; @printf 'origin=[%s] flavor=[%s] value=[%s]\n' '$(origin SHELL)' '$(flavor SHELL)' '$(value SHELL)' > out
something-else-holds-it: ; @printf 'branch=[%s]\n' '$(flavor SHELL)' > out
