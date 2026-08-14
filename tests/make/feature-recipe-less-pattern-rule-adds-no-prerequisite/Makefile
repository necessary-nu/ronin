# A pattern rule with prerequisites and no recipe is passed over by the search
# entirely, so it adds nothing to a target another rule makes: extra.h is no
# prerequisite of foo.gen however early the rule naming it was written.
%.gen: extra.h
%.gen: %.src ; @printf 'from %s\n' '$^' > $@

all: foo.gen
