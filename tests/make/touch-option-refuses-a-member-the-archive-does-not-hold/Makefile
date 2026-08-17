# A member's date lives in the archive's index, so `-t` on one is a write into
# that index rather than a `utime` — and there is nowhere to write it when the
# index holds no such member. GNU Make's `ar_member_touch` fails, the run ends,
# and no file named `libx.a(absent.o)` is invented on the way.
all: libx.a(absent.o)
libx.a(absent.o): ; @echo REAL > recipe-ran
