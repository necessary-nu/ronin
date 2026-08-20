# A recipe line with no shell syntax outside its quotes is launched as an argv
# rather than through a shell, and a scheduler-bound name is not something that
# decision can be made about: whichever process runs the line, the substitution
# has to have happened first, and it lands only where a shell runs. `$?` reaches
# a name of its own here and so do its two path halves, so the test for one is
# the prefix all three share. Getting it wrong makes a file called after the
# placeholder instead of after the prerequisite.
out: d/a
	@touch '$(?F)'
	@printf 'D=[%s] F=[%s]\n' '$(?D)' '$(?F)' > out
