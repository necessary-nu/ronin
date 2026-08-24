# GNU Make leaves `$@`'s directory to the recipe. Here the recipe's own first
# job is to clear the way — `afile` is an ordinary file and `afile/one.mk`
# cannot exist until it is gone — so a tool that creates the output's directory
# before launching the recipe stops on the name the recipe was about to remove.
# The fragment is then included, the read restarts, and `X` reaches the goal.
all: ; @echo "X=$(X)" > result
afile/one.mk: ; @echo GEN; rm -f afile; mkdir afile; echo X=1 > afile/one.mk
include afile/one.mk
