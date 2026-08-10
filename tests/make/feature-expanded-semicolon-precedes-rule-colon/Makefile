# GNU Make cuts a semicolon produced by the word being expanded before it
# searches that expansion for a rule colon. A colon in the would-be command
# therefore cannot turn this line into a rule.
RULE = target ; @printf '%s\n' wrong > wrong-out : fake

$(RULE)
