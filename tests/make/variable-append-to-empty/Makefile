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
empty_simple :=
empty_simple +=
all: ; @printf '[%s]\n' '$(recur)' '$(simple)' '$(recur_empty)' '$(simple_empty)' '$(empty_recur)' '$(empty_simple)' > out
