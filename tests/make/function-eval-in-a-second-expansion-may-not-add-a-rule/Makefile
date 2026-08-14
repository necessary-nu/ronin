# A second expansion happens after the rules are resolved too, so a rule reached
# through one is refused for the same reason a recipe's is.
.SECONDEXPANSION:
RULE = $(eval late: earlier)
all: $$(RULE)
	@printf '%s\n' 'ran' > out
