# On the immediate path the held-aside directory is part of the name before the
# glob runs, so the wildcard is matched one directory down.
all: lib/bye.x

%.x: 6%* ; @printf '%s\n' '$(sort $^)' > log; touch $@
