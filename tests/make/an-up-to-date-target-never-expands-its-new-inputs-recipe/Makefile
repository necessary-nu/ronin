# GNU Make expands a recipe in `new_job`, immediately before running it and
# only for a target it decided to remake. A target that is already up to date
# has none of it happen: the `$(error)` never fires, the `$(shell)` never runs,
# and the build succeeds having written nothing.
#
# A recipe naming `$?` is no different there — GNU reads `$?` in the same
# `new_job`, off the timestamps that are on disk by then. A tool that reads
# such a recipe while it builds the graph to bind `$?` for a scheduler fires
# the `$(error)` for a target it is not going to make.
out: a
	@printf 'q=[%s]\n' '$?' > witness $(error this recipe should never be expanded)
