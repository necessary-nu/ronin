FOO := foo
BAR := bar
ONE := $(sort $(filter FOO BAR BAZ,$(.VARIABLES)))
undefine BAR
BAZ := baz
TWO := $(sort $(filter FOO BAR BAZ,$(.VARIABLES)))
all: ; @printf '[%s]\n' '$(ONE)' '$(TWO)' > out
