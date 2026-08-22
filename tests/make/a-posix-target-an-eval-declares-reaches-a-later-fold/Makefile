# `$(eval)` is read by GNU Make where it stands, so the `.POSIX:` it evaluates is
# in force for the line below it. Here the eval runs during the evaluation, after
# the whole file has been read.
$(eval .POSIX:)

A = a   \
  b

all: ; @printf '[%s]\n' '$(A)' > out
