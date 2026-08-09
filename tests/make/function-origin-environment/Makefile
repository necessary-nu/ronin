# The harness runs every case with LC_ALL=C in the environment and nothing else
# of its own, so that is the one variable a case can call `environment`.
all: ; @printf '%s\n' '$(origin LC_ALL)' '$(LC_ALL)' > out
