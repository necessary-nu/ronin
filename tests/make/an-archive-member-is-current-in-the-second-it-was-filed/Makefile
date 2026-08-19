# An archive index dates its members in whole seconds, so a member filed from an
# object written part way through a second is dated a fraction BEFORE the object
# it is a copy of. Read plainly it is always stale, and the archive is rewritten
# on every invocation forever.
#
# GNU Make marks such a file `low_resolution_time` and rounds it up to the end
# of its second — in `update_file_1`, applied to `this_mtime`, the mtime of the
# file being UPDATED (reference/gnumake/src/remake.c: `this_mtime +=
# FILE_TIMESTAMPS_PER_S - 1 - ns`). It is a reading and not a timestamp: the
# same member keeps its plain date wherever something else reads it as a
# prerequisite, which is what keeps the archive newer than what it holds.
#
# `libcur.a(cur.o)` is the member read both ways at once. As the target of the
# `(%): %` rule it is rounded up, so `cur.o` at .7 of the same second does not
# make it stale; as a prerequisite of `libcur.a` it keeps the plain date, so the
# archive at .9 is newer than what it holds and nothing runs.
#
# `libold.a(old.o)` is the control, five whole seconds apart, where no rounding
# can settle the question either way.
all: libcur.a libold.a

libcur.a: libcur.a(cur.o)
	@echo "refiled libcur.a [$?]" >> log

libold.a: libold.a(old.o)
	@echo "refiled libold.a [$?]" >> log

(%): %
	@echo "filed $% into $@" >> log
