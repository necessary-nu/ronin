# The same line without `.POSIX:`, which is the control: the three spaces in
# front of the backslash go, and one space is what is left.
A = a   \
  b
all: ; @printf '%s' '$(A)' | od -An -c > out
