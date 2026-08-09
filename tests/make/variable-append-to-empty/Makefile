recur =
recur += foo
simple :=
simple += bar
recur_empty = foo
recur_empty +=
simple_empty := bar
simple_empty +=
empty_recur =
empty_recur +=
all: ; @printf '[%s]\n' '$(recur)' '$(simple)' '$(recur_empty)' '$(simple_empty)' '$(empty_recur)' > out
