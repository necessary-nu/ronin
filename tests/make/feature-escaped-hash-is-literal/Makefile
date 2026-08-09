# A backslash before a hash keeps it out of the comment, in a variable's value
# and in a target's name alike.
literal := a\#b

all: out\#name ; @printf '%s\n' '$(literal)' > out

out\#name: ; @printf '%s\n' '$@' > '$@'
