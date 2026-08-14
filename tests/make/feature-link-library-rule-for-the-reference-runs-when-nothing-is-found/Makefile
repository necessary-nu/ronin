# The other half of that: a search that finds nothing leaves the name as
# written, and then the rule for it is an ordinary rule and makes it.
.LIBPATTERNS = mtest_%.a
all: -lfoo ; @printf '%s\n' 'from $<' > out
-lfoo: ; @printf '%s\n' 'the rule ran' > rule.out
