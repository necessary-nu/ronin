all: ; @printf '%s\n' '$(realpath /)' '$(if $(filter $(CURDIR),$(realpath .)),same,differ)' '$(if $(filter $(CURDIR),$(realpath ./)),same,differ)' '[$(realpath nosuchfile)]' > out
