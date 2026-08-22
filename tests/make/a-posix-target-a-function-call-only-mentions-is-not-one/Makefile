# `check_specials` (read.c) reads the EXPANDED target names, and this line
# expands to nothing at all — the text `.POSIX:` is one of $(info)'s arguments
# rather than a target. A read that took the words as written would turn every
# continuation below it POSIX in a makefile that never asked.
$(info .POSIX:)

A = a   \
  b

all: ; @printf '[%s]\n' '$(A)' > out
