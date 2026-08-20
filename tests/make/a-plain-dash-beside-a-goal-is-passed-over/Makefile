# The dash is passed over wherever it stands, so the goals either side of it are
# the whole list — the same guard that makes `make -` build the default goal
# makes `make - second -` build `second` alone.
all: ; @printf 'built=[all]\n' >> out
second: ; @printf 'built=[second] goals=[%s]\n' '$(MAKECMDGOALS)' > out
