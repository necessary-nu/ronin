# GNU Make runs this search from `f_mtime`, which asks about the file before
# anything asks about the rule. So a rule written for the reference itself does
# not stop the search: the prerequisite moves to the library that was found,
# that file is current, and the rule never runs.
.LIBPATTERNS = mtest_%.a
all: -lfoo ; @printf '%s\n' 'from $<' > out
-lfoo: ; @printf '%s\n' 'the rule ran' > rule.out
