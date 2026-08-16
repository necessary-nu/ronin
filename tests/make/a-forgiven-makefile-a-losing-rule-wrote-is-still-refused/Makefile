# The verdict is about the update rather than about the bytes. A recipe that
# writes the file and then exits non-zero leaves it on disk, and GNU Make still
# refuses over it when a goal asks: the file was tried and not made, and that
# outranks what the filesystem says about the name.
-include one.mk
all: one.mk ; @printf 'all ran\n' >> log
one.mk: ; @printf 'X = 1\n' > one.mk; printf 'tried\n' >> log; exit 1
