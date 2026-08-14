# `export NAME ?= value` is one definition carrying one answer, not a directive
# beside an assignment: nothing has named NAME before the `?=` decides.
R = base
export REC = r-$(R)
export SIM := s-$(R)
export APP = a
export APP += b
export CND ?= c
R = changed
all: ; @printf 'REC=%s SIM=%s APP=%s CND=%s\n' "$$REC" "$$SIM" "$$APP" "$$CND" > out
