# Passing the pair over withdraws the pattern rule and nothing else: the rule
# still names a file called `.baz.biz`, and asking for it runs the recipe with
# the prerequisites as written.
.POSIX:
.SUFFIXES:
.SUFFIXES: .biz .baz

.baz.biz: foo.bar ; @printf '%s\n' 'made $@ from $<' > out
