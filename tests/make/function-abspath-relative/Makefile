# The answer is an absolute path that differs per checkout, so what reaches the
# file is whether it agrees with CURDIR rather than what it is.
all: ; @printf '%s\n' '$(if $(filter $(CURDIR),$(abspath .)),same,differ)' '$(if $(filter $(CURDIR),$(abspath ./)),same,differ)' '$(if $(filter $(CURDIR),$(abspath .///)),same,differ)' > out
