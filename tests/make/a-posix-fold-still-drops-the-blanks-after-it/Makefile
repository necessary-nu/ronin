# Only the blanks written BEFORE the backslash are `.POSIX:`'s. The ones on the
# far side of the newline are skipped before either reading asks anything, so
# they are gone in both.
.POSIX:
A = a\
      b
all: ; @printf '%s' '$(A)' | od -An -c > out
