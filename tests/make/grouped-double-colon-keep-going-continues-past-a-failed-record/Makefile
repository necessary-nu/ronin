# A grouped double-colon record is one action over several targets, and the
# chain between records is the same ordering as an ungrouped one, so `-k`
# reaches the second record after the first has failed.
a b &:: n1
	@printf '%s\n' first >> log
	@false
a b &:: n2
	@printf '%s\n' second >> log
