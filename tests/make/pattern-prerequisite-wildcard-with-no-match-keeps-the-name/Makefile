# A wildcard matching nothing is kept exactly as it was written rather than
# dropped, so the prerequisite is a name the search then has to account for.
all: a.four

%.four: %.t* ; @printf '%s\n' '$^' > log; touch $@

.PHONY: a.t*
