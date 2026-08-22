# The same across the makefiles `-f` names: `.POSIX:` in the first is read
# before the second is, so the second folds the POSIX way.
A = a   \
  b
all: ; @printf '%s' '$(A)' | od -An -c > out
