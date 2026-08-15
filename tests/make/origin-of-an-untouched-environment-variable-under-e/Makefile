# The environment is read before the switches are decoded, so `-e` changes
# which definition wins and not what a variable says it is. Nothing here
# writes to LC_ALL, so nothing ever asks `-e` to decide.
all: ; @printf '%s\n' '$(origin LC_ALL)' '$(LC_ALL)' > out
