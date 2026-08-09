# A pattern rule written again with no recipe withdraws the earlier one, so
# nothing knows how to make the target any more.
%.tex: %.w ; @printf '%s\n' built > $@
%.tex: %.w

all: foo.tex
