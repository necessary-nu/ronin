# Half of an odd run survives into the value, and what survives is not a blank:
# the discarding stops at it in both readings, so this is where they agree.
.POSIX:
A = a  \\\
  b
all: ; @printf '%s' '$(A)' | od -An -c > out
