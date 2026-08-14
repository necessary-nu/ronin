# A target-specific assignment that says nothing about exporting takes the
# global binding's answer, so the value is the target's and the export is the
# makefile's.
export V = global
all: ; @printf 'V=%s\n' "$$V" > out
all: V = local
