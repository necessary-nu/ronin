# The empty word is passed over wherever it stands — before the goal and after
# it — and the goal it stands beside is chosen as though it were alone. The
# default goal is left where it is.
all: ; @printf 'built=[all]\n' >> out
second: ; @printf 'built=[second] goals=[%s]\n' '$(MAKECMDGOALS)' > out
