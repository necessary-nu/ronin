# `&:` says one recipe makes all of these targets, so it runs once however many
# of them are wanted, and the rest count as made.
all: alpha beta

alpha beta &: ; @printf 'ran for %s\n' '$@' >> log; printf 'a\n' > alpha; printf 'b\n' > beta
