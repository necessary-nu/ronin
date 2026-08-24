# The `$` takes the fold's space as its name either way; what `.POSIX:` changes
# is whether the second fold's space survives the reference as value text. Here
# the flag is set by an eval, after the line had been read.
$(eval .POSIX:)
A = x$\
  \
  y
all: ; @printf '%s' '$(A)' | od -An -c > out
