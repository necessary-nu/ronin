# GNU Make defines each of these with `define_variable_cname (..., 0)` — the
# trailing zero is the recursive flag — so all four are SIMPLE, and each stands
# at the rank its own call names: MAKELEVEL at the environment's,
# MAKE_VERSION and MAKECMDGOALS at default, CURDIR at file.
#
# Only `$(flavor)` and `$(value)` can see the flavour, because for these names
# a recursive binding holds the same text its expansion produces. That is
# enough to branch a build on, which is what this case does.
ifeq ($(flavor MAKE_VERSION),simple)
deliver: worked-out-simple
else
deliver: worked-out-recursive
endif

worked-out-simple: ; @printf 'version=[%s,%s] level=[%s,%s] goals=[%s,%s] curdir=[%s,%s]\n' '$(origin MAKE_VERSION)' '$(flavor MAKE_VERSION)' '$(origin MAKELEVEL)' '$(flavor MAKELEVEL)' '$(origin MAKECMDGOALS)' '$(flavor MAKECMDGOALS)' '$(origin CURDIR)' '$(flavor CURDIR)' > out
worked-out-recursive: ; @printf 'branch=[recursive]\n' > out
