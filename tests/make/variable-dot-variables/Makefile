X1 := $(sort $(filter FOO BAR,$(.VARIABLES)))
FOO := foo
X2 := $(sort $(filter FOO BAR,$(.VARIABLES)))
BAR := bar
all: ; @printf '[%s]\n' '$(X1)' '$(X2)' '$(sort $(filter FOO BAR,$(.VARIABLES)))' > out
