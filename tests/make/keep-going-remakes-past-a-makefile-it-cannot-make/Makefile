# Without -k the first required makefile nothing can make ends the run from
# inside the update, so every makefile the read reached after it is never
# considered — that is `complain()` choosing `fatal` (remake.c:422). With -k it
# chooses `error` instead: the update reports over nope1.mk, reports over
# nope2.mk, and still brings gen.mk up to date, which is what this gates. The
# run ends 2 either way and the goal never runs.
gen.mk: ; @printf 'Z=1\n' > gen.mk
include nope1.mk
include nope2.mk
include gen.mk
all: ; @printf 'all\n' > ran
