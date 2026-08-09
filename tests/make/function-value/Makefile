FOO = foo
recurse = FOO is $$FOO and $(FOO)
static := FOO is $(value FOO)
all: ; @printf '[%s]\n' '$(recurse)' '$(value recurse)' '$(static)' '$(value static)' > out
