# A recipe naming `$?` is expanded when GNU Make is about to run it, in
# `new_job`, and only for a target it decided to remake. A `$(shell)` such a
# recipe holds therefore runs only if the target was out of date. A target
# already up to date runs none of the expansion — the shell never starts, and
# nothing it would have written appears.
out: a
	@printf 'q=[%s]\n' '$?' > witness $(shell touch ran)
