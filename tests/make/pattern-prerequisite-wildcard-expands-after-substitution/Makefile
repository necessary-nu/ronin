# A pattern rule's prerequisite may hold shell wildcards. They are expanded
# after the stem goes in rather than where the rule was read, so `%.t*` is
# looked up as `a.t*` and stands for every file it matches.
all: a.four

%.four: %.t* ; @printf '%s\n' '$(sort $^)' > log; touch $@
