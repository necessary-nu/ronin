# CURDIR is an absolute path that differs per checkout, so what reaches the file
# is what can be said about it rather than what it is.
all: ; @printf '%s\n' '$(if $(filter $(CURDIR),$(shell pwd)),same,differ)' '$(if $(patsubst /%,,$(CURDIR)),relative,absolute)' > out
