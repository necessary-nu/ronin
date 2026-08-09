FOO = foo
define recur =
@printf '%s\n' '$(FOO)' > recur.out
endef
define simple :=
@printf '%s\n' '$(FOO)' > simple.out
endef
FOO = there
all: r s
r: ; $(recur)
s: ; $(simple)
