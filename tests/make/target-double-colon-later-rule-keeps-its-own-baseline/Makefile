# Every rule in the chain is weighed against the target as it stood before any
# of them ran. An earlier recipe making the target newer than a later rule's
# prerequisite therefore does not settle that later rule.
out:: first.in
	@printf '%s\n' first >> log
	@touch -d '2030-01-01 00:00' out
out:: second.in
	@printf '%s\n' second >> log
