# A pattern-specific assignment can carry `export` too, and then it reaches
# every target the pattern matched and nothing else.
all: foo.x ; @printf 'all=%s\n' "$$PS" > all-out
%.x: export PS = ps
foo.x: ; @printf 'foo=%s\n' "$$PS" > foo-out
