# A member's timestamp comes out of the archive's index, not off a file of that
# name. Here the archive was written with `ar -U`, which records real dates, so
# the member is as new as the object beside it and there is nothing to do.
#
# The default is the other way round and deliberately so: `ar` on this host is
# deterministic, writing every member's date as zero, and GNU Make reads a zero
# date as no such member — so a plain `ar -rv` archive has its members refiled
# on every run. Both tools agree either way, because both are reading the same
# rule out of the same index.
AR = ./fake-ar
all: libx.a(foo.o)
