# Whether a Makefile's failure is forgiven and which switches it is made under
# are two questions with two answers. `-include` decides the first
# (`file->dontcare = ANY_SET (g->flags, RM_DONTCARE)`), and being named on the
# command line decides only the second (`file->cmd_target` restores -t, -q and
# -n and nothing else). So a file that is both is still forgiven, and its
# neighbour is still attempted after it loses.
-include one.mk
-include two.mk
all: ; @printf 'all ran\n' >> log
one.mk: ; @printf 'one\n' >> log; exit 1
two.mk: ; @printf 'two\n' >> log; exit 1
