# A pattern rule with several target patterns already makes them together, so
# `&:` on one is accepted and changes nothing.
all: alpha.q beta.q

alpha.% beta.%&: ; @printf 'stem=%s\n' '$*' >> log; printf 'x\n' > alpha.q; printf 'y\n' > beta.q
