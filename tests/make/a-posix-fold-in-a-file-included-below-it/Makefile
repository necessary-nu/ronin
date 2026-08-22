# The flag belongs to the read rather than to the file: a line an `include`
# brings in after `.POSIX:` was read is folded the POSIX way.
.POSIX:
include inc.mk
all: ; @printf '%s' '$(A)' | od -An -c > out
