# A double-colon chain is walked one entry at a time: a later rule for the same
# target does not start until the earlier one has finished, even under -j.
out:: first.in
	@printf '%s\n' one-start >> log
	@sleep 1
	@printf '%s\n' one-end >> log
out:: second.in
	@printf '%s\n' two-start >> log
	@printf '%s\n' two-end >> log
