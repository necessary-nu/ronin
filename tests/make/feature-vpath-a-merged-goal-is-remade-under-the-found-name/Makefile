# A goal reaches the search too, and the same merge happens: the recipe that
# runs is the found target's and the file it makes is the found path.
vpath %.te d/

fail.te: ; @printf 'here [%s]\n' '$@' > seen2
d/fail.te: ; @printf 'there [%s]\n' '$@' > seen3
