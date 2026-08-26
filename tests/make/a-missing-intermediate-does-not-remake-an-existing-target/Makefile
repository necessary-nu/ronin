# A prerequisite the implicit search invented is an intermediate file, and GNU
# Make does not remake a target that already exists just because one of those is
# missing. `update_file_1` (remake.c) forces a remake for a prerequisite that
# does not exist only `if (d_mtime == NONEXISTENT_MTIME && !d->file->intermediate)`,
# and `check_dep` reads a missing intermediate by walking through it: what
# decides the answer is whether anything BELOW it is newer than the target.
#
# Which prerequisites count as invented is settled per raw word, before any
# expansion: `is_explicit` is `strchr (nptr, '%') == 0` on the word as written
# (implicit.c), and every name the word goes on to expand to inherits it. That
# is the half `.SECONDEXPANSION` reaches — `%$$(zdep)` holds a `%`, so what it
# expands to is invented, exactly as a plain `%.x` would be.
#
# Three targets, one for each answer:
#
#   kept.z    exists; its own prerequisite `kept.x` is missing and invented, so
#             nothing happens.
#   older.tsk exists and is newer than `older.x`; the intermediate `older.z`
#             between them is missing, and walking through it finds nothing
#             newer, so nothing happens.
#   newer.tsk exists and is OLDER than `newer.x`, so the chain does run and both
#             `newer.z` and `newer.tsk` are made. `newer.z` is intermediate and
#             is swept up afterwards.
.SECONDEXPANSION:
zdep := .x
tdep := .z

all: kept.z older.tsk newer.tsk

%.z: %$$(zdep) ; @printf 'made %s\n' '$@' >> log
%.tsk: %$$(tdep) ; @printf 'made %s\n' '$@' >> log
%.x: ; @printf 'made %s\n' '$@' >> log
