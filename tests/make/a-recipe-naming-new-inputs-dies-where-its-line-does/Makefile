# GNU Make runs each command line of a recipe as its own process, so a line
# whose shell syntax is left open dies where it stands instead of being
# completed by the line after it. That holds for a recipe naming `$?` like any
# other: `set_file_variables` reads `$?` off the timestamps in `new_job`,
# immediately before the first line runs, so there is nothing about `$?` that
# needs the recipe read any earlier.
#
# The evidence is that `out` is never written. A tool that joins the two lines
# into one script puts the join's own bytes inside the quote, where they become
# data, and the command parses, succeeds, and writes a file GNU Make never
# writes.
out: a b
	@printf '%s' 'x$?
	y' > out
