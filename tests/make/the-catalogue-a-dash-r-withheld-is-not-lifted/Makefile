# The promotion belongs to the write and not to the name, which `-R` makes
# visible: `define_default_variables` (default.c) returns without defining
# anything when `no_builtin_variables_flag` is set, so under `-e -R` an
# inherited CC is never defined over and keeps its plain `environment` rank —
# while MAKE_VERSION, which `define_automatic_variables` writes outside the
# catalogue, is lifted exactly as it is without `-R`.
ifeq ($(origin MAKE_VERSION),environment override)
all: the-catalogue-was-withheld
else
all: the-catalogue-was-installed
endif

the-catalogue-was-withheld: ; @printf 'cc=[%s,%s] ver=[%s,%s]\n' '$(origin CC)' '$(CC)' '$(origin MAKE_VERSION)' '$(MAKE_VERSION)' > out
the-catalogue-was-installed: ; @printf 'branch=[%s]\n' '$(origin MAKE_VERSION)' > out
