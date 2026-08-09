# The same rule read on a special target, and with the expansion producing a
# bare `=` rather than a name containing one. It is a prerequisite named `=`
# and nothing about the line is an assignment.
EQ = =

all: ; @printf '%s\n' goal > out

.PHONY: all $(EQ)
