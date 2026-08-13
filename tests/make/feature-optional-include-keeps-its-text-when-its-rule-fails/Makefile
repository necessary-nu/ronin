# The optional include is there but out of date, and the rule that would have
# refreshed it fails. GNU Make goes on with the text it already read rather
# than stopping, so the goal is built from the old value.
all: ; @printf 'v=%s\n' '$(V)' > out

-include gen.mk

gen.mk: gen.in ; @printf 'gen ran\n' >> log; exit 1
