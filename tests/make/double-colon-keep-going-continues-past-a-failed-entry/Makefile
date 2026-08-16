# Under `-k` GNU Make walks a double-colon chain past an entry that failed and
# runs the target's later entries. `update_file` returns early only when the
# entry is still running; on a failed one it abandons the rest of the chain
# solely when `keep_going_flag` is clear (reference/gnumake/src/remake.c). The
# target itself is still not remade — the failure stands — but each entry is
# its own rule and the later ones are still reached.
out:: n1
	@printf '%s\n' first >> log
	@false
out:: n2
	@printf '%s\n' second >> log
out:: n3
	@printf '%s\n' third >> log
