# The same appends with the names given on the command line: an override append
# extends the command line's value rather than replacing or being ignored.
X += x1
override X += x2
override Y += y1
Y += y2

all: ; @printf '%s|%s\n' '$(X)' '$(Y)' > out
