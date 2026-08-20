# `$(?D)` and `$(?F)` are the directory and file halves of `$?`, one word each
# per prerequisite it names, and GNU Make binds them from the same list in
# `set_file_variables`. A tool that leaves a placeholder in the command for a
# scheduler to fill in has nothing for the halves to be taken off, and answers
# about the placeholder instead.
out: d/a b
	@printf 'q=[%s]\nD=[%s]\nF=[%s]\n' '$?' '$(?D)' '$(?F)' > out
