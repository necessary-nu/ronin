# What a target that reads a remade Makefile compares against is the Makefile's
# timestamp, not the rule behind it. This rule is out of date whenever it is
# looked at, because its prerequisite is phony, and it leaves the file alone —
# so `copy`, which is newer than gen.mk, is current.
.PHONY: force

include gen.mk

gen.mk: force ; @printf 'gen ran\n' >> log

copy: gen.mk ; @printf 'copy ran\n' >> log; cp gen.mk copy
