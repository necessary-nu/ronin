all: ; @printf 'FOO=[%s] OVR=[%s]\n' '$(FOO)' '$(MAKEOVERRIDES)' > child.out
