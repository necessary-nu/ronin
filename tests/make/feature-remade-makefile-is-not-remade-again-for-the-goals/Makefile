# Bringing a Makefile up to date is work the rest of the run has already had
# done for it. This rule is out of date whenever it is looked at, because its
# prerequisite is phony, and it leaves the Makefile's timestamp alone, so the
# read is not started again — and the goal that names the same Makefile gets the
# copy that was just made rather than a second run of the recipe.
.PHONY: force

include gen.mk

gen.mk: force ; @printf 'ran\n' >> log

all: gen.mk ; @printf 'done\n' > out
