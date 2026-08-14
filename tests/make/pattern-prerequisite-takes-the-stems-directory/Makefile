# A pattern rule written for bare names that matched a name carrying a
# directory holds the directory aside: only what is left of the stem goes where
# the `%` stands, and the directory goes in front of the whole prerequisite. A
# prerequisite naming no stem takes neither.
all: lib/bye.x

%.x: 6% 11%12 %9 sub/%.c nopercent.c ; @printf '%s\n' '$^' > log; printf 'stem=%s\n' '$*' >> log; touch $@
