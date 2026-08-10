# A source word existed before the literal semicolon, so its empty expansion
# makes the whole apparent rule and inline recipe disappear.
EMPTY =
$(EMPTY); @false

.PHONY: all
all: ; @printf '%s\n' ok > all-out
