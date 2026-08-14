WHERE := $(WHO)-then-second
all: ; @printf '%s\n' '$(WHERE) [$(MAKEFILE_LIST)]' > out
