# Appending under override keeps the override; appending without it to a name
# already overridden is dropped.
X += x1
override X += x2
override Y += y1
Y += y2

all: ; @printf '%s|%s\n' '$(X)' '$(Y)' > out
